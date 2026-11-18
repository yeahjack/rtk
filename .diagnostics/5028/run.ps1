$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
$originalPath = $env:PATH
$git = (Get-Command git.exe).Source
$probe = (Resolve-Path '.diagnostics/5028/probe/target/debug/path-5028-probe.exe').Path
$work = Join-Path $env:RUNNER_TEMP ('path-5028-' + [guid]::NewGuid())
New-Item -ItemType Directory $work | Out-Null
$env:HOME = Join-Path $work 'home'
$env:XDG_CONFIG_HOME = Join-Path $work 'config'
$env:XDG_DATA_HOME = Join-Path $work 'data'
$env:GIT_CONFIG_NOSYSTEM = '1'
$env:GIT_CONFIG_GLOBAL = Join-Path $work 'empty.gitconfig'
New-Item -ItemType File $env:GIT_CONFIG_GLOBAL | Out-Null
$identity = [Security.Principal.WindowsIdentity]::GetCurrent().User
$repo = Join-Path $work 'repo'
& $git init -q $repo
if ($LASTEXITCODE) { throw 'git init failed' }
$rtks = @()
foreach ($tag in @('v1.1.0','v1.2.0')) {
  $release = gh api "repos/rtk-ai/rtk/releases/tags/$tag" | ConvertFrom-Json
  if ($LASTEXITCODE) { throw 'release lookup failed' }
  $asset = $release.assets | Where-Object name -EQ 'rtk-x86_64-pc-windows-msvc.zip'
  $zip = Join-Path $work "$tag.zip"
  Invoke-WebRequest $asset.browser_download_url -OutFile $zip
  $hash = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLowerInvariant()
  if ($asset.digest -ne "sha256:$hash") { throw 'release digest mismatch' }
  Expand-Archive $zip (Join-Path $work $tag)
  $rtks += (Get-ChildItem (Join-Path $work $tag) -Recurse -Filter rtk.exe).FullName
  Write-Output "Verified $tag SHA256 $hash"
}
Set-Location $repo
foreach ($kind in @('control','metadata-denied','execute-denied','read-denied')) {
  $bad = Join-Path $work $kind
  New-Item -ItemType Directory $bad | Out-Null
  $file = Join-Path $bad 'git.exe'
  # Exact copy of installed Git launcher; denied variants never execute it.
  if ($kind -ne 'control') { Copy-Item $git $file }
  $acl = $null
  try {
    if ($kind -ne 'control') {
      $acl = Get-Acl $file
      $rights = switch ($kind) {
        'metadata-denied' { [Security.AccessControl.FileSystemRights]::ReadAndExecute }
        'execute-denied' { [Security.AccessControl.FileSystemRights]::ExecuteFile }
        'read-denied' { [Security.AccessControl.FileSystemRights]::ReadData }
      }
      $denied = Get-Acl $file
      $rule = [Security.AccessControl.FileSystemAccessRule]::new($identity,$rights,[Security.AccessControl.AccessControlType]::Deny)
      $denied.AddAccessRule($rule)
      Set-Acl $file $denied
    }
    $env:PATH = "$bad;$originalPath"
    Write-Output "=== $kind restricted candidate FIRST ==="
    & $probe $bad
    foreach ($rtk in $rtks) {
      & $rtk --version
      & $rtk git status 2>&1 | ForEach-Object { Write-Output $_ }
      Write-Output "rtk git status exit=$LASTEXITCODE"
    }
    $env:PATH = "$originalPath;$bad"
    Write-Output "=== $kind restricted candidate LAST ==="
    & $probe $bad
    foreach ($rtk in $rtks) {
      & $rtk git status 2>&1 | ForEach-Object { Write-Output $_ }
      Write-Output "rtk git status exit=$LASTEXITCODE"
    }
  } finally {
    $env:PATH = $originalPath
    if ($acl) { Set-Acl $file $acl }
  }
}
# runner is disposable; retain files only until the job is destroyed.
