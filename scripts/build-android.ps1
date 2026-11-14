param(
    [string]$SdkPath = "$env:USERPROFILE\.cache\folio-android\sdk",
    [string]$GradlePath = "$env:USERPROFILE\.cache\folio-android\gradle-8.11.1\bin\gradle.bat",
    [string]$JavaPath = 'C:\Program Files\Java\jdk-17',
    [string]$GoogleMaven = 'https://maven.aliyun.com/repository/google',
    [switch]$SkipWebBuild
)
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path $PSScriptRoot -Parent
if (!(Test-Path -LiteralPath "$SdkPath\platforms\android-35\android.jar")) { throw "Android SDK 35 not found: $SdkPath" }
if (!(Test-Path -LiteralPath $GradlePath)) { throw "Gradle 8.11.1 not found: $GradlePath" }
$env:JAVA_HOME = $JavaPath
$env:ANDROID_HOME = $SdkPath

if (!$SkipWebBuild) {
    Push-Location "$projectRoot\android\web"
    try {
        if (!(Test-Path node_modules)) { & npm.cmd ci; if ($LASTEXITCODE) { throw 'npm ci failed' } }
        & npm.cmd run build
        if ($LASTEXITCODE) { throw 'Web editor build failed' }
    } finally { Pop-Location }
}
if (!(Test-Path "$projectRoot\android\web\dist\index.html")) { throw 'Missing web editor dist/index.html' }
Push-Location "$projectRoot\android"
try {
    & $GradlePath --no-daemon --console=plain "-PfolioGoogleMaven=$GoogleMaven" :app:assembleDebug
    if ($LASTEXITCODE) { throw 'Android build failed' }
} finally { Pop-Location }

$destination = "$projectRoot\dist\android"
New-Item -ItemType Directory -Force $destination | Out-Null
Copy-Item -LiteralPath "$projectRoot\android\app\build\outputs\apk\debug\app-debug.apk" -Destination "$destination\Folio-Android-0.1.0.apk" -Force
Write-Host "APK: $destination\Folio-Android-0.1.0.apk"
