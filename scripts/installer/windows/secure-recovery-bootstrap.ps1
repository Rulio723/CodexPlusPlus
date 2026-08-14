$ErrorActionPreference=1
$ProgressPreference=0
$n=+$env:CODEXPP_RECOVERY_SCRIPT_COUNT
if($n-lt1-or$n-gt64){throw}
$b=''
0..($n-1)|%{$b+=(gi "env:CODEXPP_RECOVERY_SCRIPT_$_").Value}
iex ([Text.Encoding]::Unicode.GetString([Convert]::FromBase64String($b)))
