$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$DefaultInstallDirectory = Join-Path $env:LOCALAPPDATA "Programs\Savestate\bin"
$InstallDirectory = if ($env:SAVESTATE_INSTALL_DIR) {
    $env:SAVESTATE_INSTALL_DIR
} else {
    $DefaultInstallDirectory
}

$InstallDirectory = [System.IO.Path]::GetFullPath($InstallDirectory)
$DefaultInstallDirectory = [System.IO.Path]::GetFullPath($DefaultInstallDirectory)
$BinaryPath = Join-Path $InstallDirectory "savestate.exe"
$PathMarker = Join-Path $InstallDirectory ".savestate-path-added"
$InstallerOwnedPath = Test-Path -LiteralPath $PathMarker -PathType Leaf
$DedicatedInstallDirectory = [string]::Equals(
    $InstallDirectory.TrimEnd("\"),
    $DefaultInstallDirectory.TrimEnd("\"),
    [System.StringComparison]::OrdinalIgnoreCase
)

if (Test-Path -LiteralPath $BinaryPath -PathType Container) {
    throw "Refusing to remove a directory at $BinaryPath."
}

if (Test-Path -LiteralPath $BinaryPath -PathType Leaf) {
    Remove-Item -LiteralPath $BinaryPath -Force
    Write-Host "Removed Savestate from $BinaryPath"
} else {
    Write-Host "Savestate was not found at $BinaryPath"
}

if ($InstallerOwnedPath) {
    Remove-Item -LiteralPath $PathMarker -Force
}

$DirectoryHasOtherItems = (Test-Path -LiteralPath $InstallDirectory -PathType Container) -and
    (@(Get-ChildItem -LiteralPath $InstallDirectory -Force).Count -gt 0)
$CanRemovePathEntry = ($InstallerOwnedPath -or $DedicatedInstallDirectory) -and -not $DirectoryHasOtherItems

if ($CanRemovePathEntry) {
    $UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
    $KeptEntries = @()
    $RemovedPathEntry = $false

    foreach ($Entry in @($UserPath -split ";" | Where-Object { $_ })) {
        try {
            $NormalizedEntry = [System.IO.Path]::GetFullPath($Entry).TrimEnd("\")
        } catch {
            $KeptEntries += $Entry
            continue
        }

        if ([string]::Equals(
            $NormalizedEntry,
            $InstallDirectory.TrimEnd("\"),
            [System.StringComparison]::OrdinalIgnoreCase
        )) {
            $RemovedPathEntry = $true
        } else {
            $KeptEntries += $Entry
        }
    }

    if ($RemovedPathEntry) {
        [Environment]::SetEnvironmentVariable("Path", ($KeptEntries -join ";"), "User")
        Write-Host "Removed $InstallDirectory from your user PATH."
    }
} elseif (($InstallerOwnedPath -or $DedicatedInstallDirectory) -and $DirectoryHasOtherItems) {
    Write-Host "Kept $InstallDirectory on PATH because the directory contains other files."
}

if ($DedicatedInstallDirectory -and
    (Test-Path -LiteralPath $InstallDirectory -PathType Container) -and
    (@(Get-ChildItem -LiteralPath $InstallDirectory -Force).Count -eq 0)) {
    Remove-Item -LiteralPath $InstallDirectory -Force
    $InstallRoot = Split-Path -Parent $InstallDirectory
    if ((Test-Path -LiteralPath $InstallRoot -PathType Container) -and
        (@(Get-ChildItem -LiteralPath $InstallRoot -Force).Count -eq 0)) {
        Remove-Item -LiteralPath $InstallRoot -Force
    }
}

Write-Host ""
Write-Host "Project checkpoints and configuration were not removed."
Write-Host "For Cargo installations, run: cargo uninstall savestate"
