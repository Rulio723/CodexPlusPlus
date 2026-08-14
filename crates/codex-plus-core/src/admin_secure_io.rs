use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, ensure};

#[cfg(windows)]
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
#[cfg(windows)]
use std::os::windows::io::{AsRawHandle, FromRawHandle};

#[derive(Debug)]
pub(crate) struct SecureFileLease {
    file: std::fs::File,
    path: PathBuf,
    #[cfg(windows)]
    _parent: std::fs::File,
}

impl SecureFileLease {
    pub(crate) fn as_file(&self) -> &std::fs::File {
        &self.file
    }

    pub(crate) fn open(path: &Path, writable: bool) -> anyhow::Result<Self> {
        #[cfg(windows)]
        return Self::open_windows(path, writable, false, false, || Ok(()));
        #[cfg(not(windows))]
        {
            let parent = pin_parent(path)?;
            let mut options = std::fs::OpenOptions::new();
            options.read(true).write(writable);
            configure_file_options(&mut options, writable);
            let file = options.open(path).with_context(|| {
                format!(
                    "failed to open trusted administrator file {}",
                    path.display()
                )
            })?;
            ensure_regular_file(&file)?;
            ensure_handle_path(&file, path)?;
            Ok(Self {
                file,
                path: path.to_owned(),
            })
        }
    }

    pub(crate) fn open_for_delete(path: &Path) -> anyhow::Result<Self> {
        #[cfg(windows)]
        return Self::open_windows(path, false, true, false, || Ok(()));
        #[cfg(not(windows))]
        {
            let parent = pin_parent(path)?;
            let mut options = std::fs::OpenOptions::new();
            options.read(true);
            configure_delete_options(&mut options);
            let file = options.open(path).with_context(|| {
                format!(
                    "failed to open trusted administrator file for deletion {}",
                    path.display()
                )
            })?;
            ensure_regular_file(&file)?;
            ensure_handle_path(&file, path)?;
            Ok(Self {
                file,
                path: path.to_owned(),
            })
        }
    }

    pub(crate) fn create(path: &Path) -> anyhow::Result<Self> {
        Self::create_impl(path, || Ok(()))
    }

    fn create_impl(
        path: &Path,
        before_open: impl FnOnce() -> anyhow::Result<()>,
    ) -> anyhow::Result<Self> {
        #[cfg(windows)]
        return Self::open_windows(path, true, false, true, before_open);
        #[cfg(not(windows))]
        {
            before_open()?;
            let parent = pin_parent(path)?;
            let mut options = std::fs::OpenOptions::new();
            options.read(true).write(true).create_new(true);
            configure_file_options(&mut options, true);
            let file = options.open(path).with_context(|| {
                format!(
                    "failed to create trusted administrator file {}",
                    path.display()
                )
            })?;
            if let Err(error) = ensure_regular_file(&file) {
                let _ = delete_file_handle(&file);
                return Err(error);
            }
            if let Err(error) = ensure_handle_path(&file, path) {
                let _ = delete_file_handle(&file);
                return Err(error);
            }
            Ok(Self {
                file,
                path: path.to_owned(),
            })
        }
    }

    #[cfg(windows)]
    fn open_windows(
        path: &Path,
        writable: bool,
        delete_only: bool,
        create_new: bool,
        before_open: impl FnOnce() -> anyhow::Result<()>,
    ) -> anyhow::Result<Self> {
        let (parent, name) = pin_parent_and_name_with_hook(path, before_open)?;
        let file = open_relative_file(&parent, &name, writable, delete_only, create_new)
            .with_context(|| {
                format!(
                    "failed to open trusted administrator file {}",
                    path.display()
                )
            })?;
        if let Err(error) = ensure_regular_file(&file) {
            if create_new {
                let _ = delete_file_handle(&file);
            }
            return Err(error);
        }
        Ok(Self {
            file,
            path: path.to_owned(),
            _parent: parent,
        })
    }

    pub(crate) fn read_all(&mut self) -> anyhow::Result<Vec<u8>> {
        ensure_regular_file(&self.file)?;
        self.file.seek(SeekFrom::Start(0))?;
        let mut bytes = Vec::new();
        self.file.read_to_end(&mut bytes)?;
        Ok(bytes)
    }

    pub(crate) fn replace_contents(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        ensure_regular_file(&self.file)?;
        self.file.seek(SeekFrom::Start(0))?;
        self.file.set_len(0)?;
        if let Err(error) = self
            .file
            .write_all(bytes)
            .and_then(|_| self.file.sync_all())
        {
            return Err(error).with_context(|| {
                format!(
                    "failed to replace trusted administrator file {}",
                    self.path.display()
                )
            });
        }
        Ok(())
    }

    pub(crate) fn delete(self) -> anyhow::Result<()> {
        ensure_regular_file(&self.file)?;
        delete_file_handle(&self.file)?;
        Ok(())
    }

    pub(crate) fn final_path(&self) -> anyhow::Result<PathBuf> {
        final_path_for_handle(&self.file)
    }

    #[cfg(windows)]
    pub(crate) fn rename_to(&mut self, destination: &Path) -> anyhow::Result<()> {
        use std::os::windows::ffi::OsStrExt;
        use windows::Win32::Storage::FileSystem::FILE_RENAME_INFO;

        ensure_regular_file(&self.file)?;
        ensure!(
            self.path.parent() == destination.parent(),
            "administrator file rename must remain in its pinned directory"
        );
        let destination_name = destination
            .file_name()
            .context("administrator rename destination has no file name")?;
        let name: Vec<u16> = destination_name.encode_wide().collect();
        let offset = std::mem::offset_of!(FILE_RENAME_INFO, FileName);
        let size = offset + name.len() * std::mem::size_of::<u16>();
        let mut buffer = vec![0usize; size.div_ceil(std::mem::size_of::<usize>())];
        let information = buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();
        unsafe {
            (*information).Anonymous.ReplaceIfExists.0 = 0;
            (*information).RootDirectory.0 = self._parent.as_raw_handle();
            (*information).FileNameLength = (name.len() * 2) as u32;
            std::ptr::copy_nonoverlapping(
                name.as_ptr(),
                std::ptr::addr_of_mut!((*information).FileName).cast::<u16>(),
                name.len(),
            );
            let mut io_status = NtIoStatusBlock {
                status_or_pointer: 0,
                information: 0,
            };
            let status = NtSetInformationFile(
                self.file.as_raw_handle(),
                &mut io_status,
                buffer.as_ptr().cast(),
                size as u32,
                10,
            );
            if status < 0 {
                let code = RtlNtStatusToDosError(status);
                return Err(std::io::Error::from_raw_os_error(code as i32).into());
            }
        }
        self.path = destination.to_owned();
        Ok(())
    }
}

pub(crate) fn ensure_directory(path: &Path) -> anyhow::Result<()> {
    if path.exists() {
        #[cfg(windows)]
        let _directory = pin_directory_with_hook(path, || Ok(()))?;
        #[cfg(not(windows))]
        {
            validate_no_reparse_ancestors(path)?;
            ensure!(path.is_dir(), "administrator state path is not a directory");
        }
        return Ok(());
    }
    let parent = path
        .parent()
        .context("administrator directory has no parent")?;
    ensure_directory(parent)?;
    #[cfg(windows)]
    let create_result = (|| {
        let (parent, name) = pin_parent_and_name_with_hook(path, || Ok(()))?;
        create_relative_directory(&parent, &name).map(|_| ())
    })();
    #[cfg(not(windows))]
    let create_result = std::fs::create_dir(path).map_err(anyhow::Error::from);
    match create_result {
        Ok(()) => {}
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::AlreadyExists) => {}
        Err(error) => return Err(error.into()),
    }
    #[cfg(windows)]
    let _directory = pin_directory_with_hook(path, || Ok(()))?;
    #[cfg(not(windows))]
    {
        validate_no_reparse_ancestors(path)?;
        ensure!(path.is_dir(), "administrator state path is not a directory");
    }
    Ok(())
}

pub(crate) fn create_new(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let mut lease = SecureFileLease::create(path)?;
    if let Err(error) = lease.replace_contents(bytes) {
        let _ = lease.delete();
        return Err(error);
    }
    Ok(())
}

#[cfg(windows)]
fn pin_parent_and_name_with_hook(
    path: &Path,
    before_components: impl FnOnce() -> anyhow::Result<()>,
) -> anyhow::Result<(std::fs::File, std::ffi::OsString)> {
    let parent = pin_directory_with_hook(
        path.parent()
            .context("administrator file has no trusted parent")?,
        before_components,
    )?;
    let name = path
        .file_name()
        .context("administrator file has no trusted name")?
        .to_os_string();
    Ok((parent, name))
}

#[cfg(windows)]
fn pin_directory_with_hook(
    path: &Path,
    before_components: impl FnOnce() -> anyhow::Result<()>,
) -> anyhow::Result<std::fs::File> {
    use std::path::{Component, Prefix};
    use windows::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_READ,
        FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    let mut components = path.components();
    let prefix = match components.next() {
        Some(Component::Prefix(prefix)) => prefix,
        _ => anyhow::bail!("administrator path must be an absolute local disk path"),
    };
    ensure!(
        matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)),
        "administrator path must be an absolute local disk path"
    );
    ensure!(
        matches!(components.next(), Some(Component::RootDir)),
        "administrator path must be absolute"
    );
    let mut root_path = PathBuf::from(prefix.as_os_str());
    root_path.push(r"\");
    let mut options = std::fs::OpenOptions::new();
    options
        .access_mode(FILE_GENERIC_READ.0)
        .share_mode(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0 | FILE_FLAG_OPEN_REPARSE_POINT.0);
    let mut directory = options.open(&root_path).with_context(|| {
        format!(
            "failed to pin administrator volume root {}",
            root_path.display()
        )
    })?;
    ensure!(
        !metadata_is_reparse(&directory.metadata()?),
        "administrator directory must not be a reparse point"
    );
    before_components()?;
    for component in components {
        let Component::Normal(name) = component else {
            anyhow::bail!("administrator path contains an untrusted component");
        };
        directory = open_relative_directory(&directory, name)?;
        ensure!(
            !metadata_is_reparse(&directory.metadata()?),
            "administrator directory must not be a reparse point"
        );
    }
    Ok(directory)
}

#[cfg(windows)]
#[repr(C)]
struct NtUnicodeString {
    length: u16,
    maximum_length: u16,
    buffer: *mut u16,
}

#[cfg(windows)]
#[repr(C)]
struct NtObjectAttributes {
    length: u32,
    root_directory: *mut std::ffi::c_void,
    object_name: *mut NtUnicodeString,
    attributes: u32,
    security_descriptor: *mut std::ffi::c_void,
    security_quality_of_service: *mut std::ffi::c_void,
}

#[cfg(windows)]
#[repr(C)]
struct NtIoStatusBlock {
    status_or_pointer: usize,
    information: usize,
}

#[cfg(windows)]
#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtCreateFile(
        file_handle: *mut *mut std::ffi::c_void,
        desired_access: u32,
        object_attributes: *const NtObjectAttributes,
        io_status_block: *mut NtIoStatusBlock,
        allocation_size: *const i64,
        file_attributes: u32,
        share_access: u32,
        create_disposition: u32,
        create_options: u32,
        ea_buffer: *const std::ffi::c_void,
        ea_length: u32,
    ) -> i32;
    fn NtSetInformationFile(
        file_handle: *mut std::ffi::c_void,
        io_status_block: *mut NtIoStatusBlock,
        file_information: *const std::ffi::c_void,
        length: u32,
        file_information_class: i32,
    ) -> i32;
    fn RtlNtStatusToDosError(status: i32) -> u32;
}

#[cfg(windows)]
fn nt_open_relative(
    parent: &std::fs::File,
    name: &std::ffi::OsStr,
    desired_access: u32,
    share_access: u32,
    create_disposition: u32,
    create_options: u32,
) -> anyhow::Result<std::fs::File> {
    use std::os::windows::ffi::OsStrExt;

    let mut name: Vec<u16> = name.encode_wide().collect();
    ensure!(
        !name.is_empty() && name.len() <= (u16::MAX as usize / 2),
        "administrator path component is invalid"
    );
    let mut unicode = NtUnicodeString {
        length: (name.len() * 2) as u16,
        maximum_length: (name.len() * 2) as u16,
        buffer: name.as_mut_ptr(),
    };
    let attributes = NtObjectAttributes {
        length: std::mem::size_of::<NtObjectAttributes>() as u32,
        root_directory: parent.as_raw_handle(),
        object_name: &mut unicode,
        attributes: 0x40,
        security_descriptor: std::ptr::null_mut(),
        security_quality_of_service: std::ptr::null_mut(),
    };
    let mut io_status = NtIoStatusBlock {
        status_or_pointer: 0,
        information: 0,
    };
    let mut handle = std::ptr::null_mut();
    let status = unsafe {
        NtCreateFile(
            &mut handle,
            desired_access,
            &attributes,
            &mut io_status,
            std::ptr::null(),
            0x80,
            share_access,
            create_disposition,
            create_options,
            std::ptr::null(),
            0,
        )
    };
    if status < 0 || handle.is_null() {
        let code = unsafe { RtlNtStatusToDosError(status) };
        return Err(std::io::Error::from_raw_os_error(code as i32).into());
    }
    Ok(unsafe { std::fs::File::from_raw_handle(handle) })
}

#[cfg(windows)]
fn open_relative_directory(
    parent: &std::fs::File,
    name: &std::ffi::OsStr,
) -> anyhow::Result<std::fs::File> {
    nt_open_relative(parent, name, 0x0010_00a0, 0x0000_0003, 1, 0x0020_0021)
}

#[cfg(windows)]
fn create_relative_directory(
    parent: &std::fs::File,
    name: &std::ffi::OsStr,
) -> anyhow::Result<std::fs::File> {
    nt_open_relative(parent, name, 0x0010_00a0, 0x0000_0003, 2, 0x0020_0021)
}

#[cfg(windows)]
fn open_relative_file(
    parent: &std::fs::File,
    name: &std::ffi::OsStr,
    writable: bool,
    delete_only: bool,
    create_new: bool,
) -> anyhow::Result<std::fs::File> {
    let mut access = 0x0010_0080 | 0x8000_0000;
    if writable {
        access |= 0x4000_0000 | 0x0001_0000;
    }
    if delete_only {
        access |= 0x0001_0000;
    }
    nt_open_relative(
        parent,
        name,
        access,
        0x0000_0001,
        if create_new { 2 } else { 1 },
        0x0020_0060,
    )
}

#[cfg(not(windows))]
fn pin_parent(path: &Path) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .context("administrator file has no trusted parent")?;
    validate_no_reparse_ancestors(parent)
}

#[cfg(not(windows))]
fn validate_no_reparse_ancestors(path: &Path) -> anyhow::Result<()> {
    let mut current = Some(path);
    while let Some(candidate) = current {
        let metadata = std::fs::symlink_metadata(candidate).with_context(|| {
            format!(
                "failed to inspect administrator directory {}",
                candidate.display()
            )
        })?;
        ensure!(
            !metadata.file_type().is_symlink(),
            "administrator directory must not be a symbolic link"
        );
        current = candidate.parent();
    }
    Ok(())
}

#[cfg(not(windows))]
fn configure_delete_options(_options: &mut std::fs::OpenOptions) {}

#[cfg(not(windows))]
fn configure_file_options(_options: &mut std::fs::OpenOptions, _writable: bool) {}

fn ensure_regular_file(file: &std::fs::File) -> anyhow::Result<()> {
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file(),
        "administrator path must be a regular file"
    );
    #[cfg(windows)]
    ensure!(
        !metadata_is_reparse(&metadata),
        "administrator file must not be a reparse point"
    );
    #[cfg(windows)]
    ensure_single_link(file)?;
    Ok(())
}

#[cfg(windows)]
fn ensure_single_link(file: &std::fs::File) -> anyhow::Result<()> {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    unsafe {
        GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut information)?;
    }
    ensure!(
        information.nNumberOfLinks == 1,
        "administrator file must not have multiple hard links"
    );
    Ok(())
}

#[cfg(windows)]
fn final_path_for_handle(file: &std::fs::File) -> anyhow::Result<PathBuf> {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{FILE_NAME_NORMALIZED, GetFinalPathNameByHandleW};
    let mut buffer = vec![0u16; 32768];
    let length = unsafe {
        GetFinalPathNameByHandleW(
            HANDLE(file.as_raw_handle()),
            &mut buffer,
            FILE_NAME_NORMALIZED,
        )
    } as usize;
    ensure!(
        length > 0 && length < buffer.len(),
        "failed to resolve administrator file handle"
    );
    buffer.truncate(length);
    let path = String::from_utf16(&buffer)?;
    Ok(PathBuf::from(path.strip_prefix(r"\\?\").unwrap_or(&path)))
}

#[cfg(not(windows))]
fn ensure_handle_path(_file: &std::fs::File, _requested: &Path) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(windows)]
fn metadata_is_reparse(metadata: &std::fs::Metadata) -> bool {
    use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0
}

#[cfg(windows)]
fn delete_file_handle(file: &std::fs::File) -> anyhow::Result<()> {
    use windows::Win32::Foundation::{BOOLEAN, HANDLE};
    use windows::Win32::Storage::FileSystem::{
        FILE_DISPOSITION_INFO, FileDispositionInfo, SetFileInformationByHandle,
    };
    let disposition = FILE_DISPOSITION_INFO {
        DeleteFile: BOOLEAN(1),
    };
    unsafe {
        SetFileInformationByHandle(
            HANDLE(file.as_raw_handle()),
            FileDispositionInfo,
            std::ptr::from_ref(&disposition).cast(),
            std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
        )?;
    }
    Ok(())
}

#[cfg(not(windows))]
fn delete_file_handle(file: &std::fs::File) -> anyhow::Result<()> {
    let _ = file;
    anyhow::bail!("secure administrator handle deletion is only available on Windows")
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    fn junction(link: &Path, target: &Path) {
        let status = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .status()
            .unwrap();
        assert!(status.success());
    }

    #[test]
    fn create_new_rejects_a_prepositioned_file_reparse_point() {
        let temp = tempfile::tempdir().unwrap();
        let outside = temp.path().join("outside");
        let target = temp.path().join("descriptor.json");
        std::fs::create_dir(&outside).unwrap();
        junction(&target, &outside);

        assert!(create_new(&target, b"admin").is_err());
        assert!(outside.is_dir());
    }

    #[test]
    fn create_new_rejects_a_reparse_parent() {
        let temp = tempfile::tempdir().unwrap();
        let outside = temp.path().join("outside");
        let linked = temp.path().join("linked");
        std::fs::create_dir(&outside).unwrap();
        junction(&linked, &outside);

        assert!(create_new(&linked.join("proof"), b"secret").is_err());
        assert!(!outside.join("proof").exists());
    }

    #[test]
    fn replacement_does_not_use_a_predictable_tmp_name() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("journal.json");
        let outside = temp.path().join("outside");
        let predictable = temp.path().join("journal.json.tmp");
        std::fs::write(&target, b"old").unwrap();
        std::fs::create_dir(&outside).unwrap();
        junction(&predictable, &outside);

        let mut lease = SecureFileLease::open(&target, true).unwrap();
        assert_eq!(lease.read_all().unwrap(), b"old");
        lease.replace_contents(b"new").unwrap();
        drop(lease);
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
        assert!(outside.is_dir());
    }

    #[test]
    fn replacement_rejects_a_target_reparse_point() {
        let temp = tempfile::tempdir().unwrap();
        let outside = temp.path().join("outside");
        let target = temp.path().join("journal.json");
        std::fs::create_dir(&outside).unwrap();
        junction(&target, &outside);

        assert!(SecureFileLease::open(&target, true).is_err());
        assert!(outside.is_dir());
    }

    #[test]
    fn replacement_rejects_a_prepositioned_hard_link() {
        let temp = tempfile::tempdir().unwrap();
        let outside = temp.path().join("outside.txt");
        let target = temp.path().join("journal.json");
        std::fs::write(&outside, b"old").unwrap();
        std::fs::hard_link(&outside, &target).unwrap();

        assert!(SecureFileLease::open(&target, true).is_err());
        assert_eq!(std::fs::read(&outside).unwrap(), b"old");
    }

    #[test]
    fn open_lease_blocks_target_write_and_replacement_until_drop() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("journal.json");
        let replacement = temp.path().join("replacement.json");
        std::fs::write(&target, b"old").unwrap();
        std::fs::write(&replacement, b"attacker").unwrap();

        let mut lease = SecureFileLease::open(&target, true).unwrap();
        assert!(std::fs::write(&target, b"attacker").is_err());
        assert!(std::fs::rename(&replacement, &target).is_err());
        lease.replace_contents(b"new").unwrap();
        drop(lease);

        assert_eq!(std::fs::read(&target).unwrap(), b"new");
    }

    #[test]
    fn lease_operations_reject_a_hard_link_added_after_open() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("administrator.json");
        let outside = temp.path().join("outside.json");
        std::fs::write(&target, b"owned").unwrap();

        let mut lease = SecureFileLease::open(&target, true).unwrap();
        std::fs::hard_link(&target, &outside).unwrap();

        assert!(lease.read_all().is_err());
        assert!(lease.replace_contents(b"secret").is_err());
        assert_eq!(std::fs::read(&outside).unwrap(), b"owned");
    }

    #[test]
    fn create_uses_the_pinned_parent_when_an_ancestor_is_swapped() {
        let temp = tempfile::tempdir().unwrap();
        let trusted = temp.path().join("trusted");
        let moved = temp.path().join("trusted-moved");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(trusted.join("nested")).unwrap();
        std::fs::create_dir_all(outside.join("nested")).unwrap();
        let target = trusted.join("nested/descriptor.json");

        assert!(
            SecureFileLease::create_impl(&target, || {
                std::fs::rename(&trusted, &moved).unwrap();
                junction(&trusted, &outside);
                Ok(())
            })
            .is_err()
        );

        assert!(!moved.join("nested/descriptor.json").exists());
        assert!(!outside.join("nested/descriptor.json").exists());
    }

    #[test]
    fn rename_keeps_destination_bound_to_the_pinned_parent() {
        let temp = tempfile::tempdir().unwrap();
        let trusted = temp.path().join("trusted");
        let moved = temp.path().join("trusted-moved");
        std::fs::create_dir_all(trusted.join("nested")).unwrap();
        let stage = trusted.join("nested/stage.json");
        let destination = trusted.join("nested/published.json");
        let mut lease = SecureFileLease::create(&stage).unwrap();
        lease.replace_contents(b"owned").unwrap();

        assert!(std::fs::rename(&trusted, &moved).is_err());
        lease.rename_to(&destination).unwrap();

        assert!(!stage.exists());
        assert_eq!(std::fs::read(destination).unwrap(), b"owned");
        assert!(!moved.exists());
    }
}
