<#
.SYNOPSIS
  Builds Pear Lite into dist\PearLite.exe (single file).
.EXAMPLE
  .\build.ps1          # release build -> dist\PearLite.exe
  .\build.ps1 -Test    # run unit tests
  .\build.ps1 -Run     # build, then start the app
#>
param([switch]$Test, [switch]$Run)
$ErrorActionPreference = 'Stop'
$root = $PSScriptRoot

# Windows PowerShell turns a native tool's stderr (cargo's progress output) into
# terminating errors when output is redirected; run native tools leniently and
# check their exit codes instead.
function Invoke-Native([string]$Exe) {
    $ErrorActionPreference = 'Continue'
    & $Exe @args
    if ($LASTEXITCODE) { throw "$Exe $args failed with exit code $LASTEXITCODE" }
}
$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw 'Rust is not installed. Get it from https://rustup.rs'
}

$hostTriple = ((rustc -vV) -match '^host: ' -replace '^host: ', '').Trim()
if ($hostTriple -like '*-windows-gnu') {
    # rustup's MinGW toolchain has two gaps for the `windows` crates:
    #  - its dlltool needs an assembler it doesn't ship  -> use LLVM's dlltool (llvm-ar in disguise)
    #  - GNU ld mislinks the resulting import libraries -> link with rust-lld via tools\ld-shim.rs
    $sysroot = (rustc --print sysroot).Trim()
    $bin = Join-Path $sysroot "lib\rustlib\$hostTriple\bin"
    if (-not (Test-Path "$bin\llvm-ar.exe")) { Invoke-Native rustup component add llvm-tools }
    $tools = Join-Path $root 'target\tools'
    New-Item -ItemType Directory -Force "$tools\lld" | Out-Null
    $dlltool = "$tools\llvm-dlltool.exe"
    if (-not (Test-Path $dlltool)) {
        try { New-Item -ItemType HardLink -Path $dlltool -Target "$bin\llvm-ar.exe" | Out-Null }
        catch { Copy-Item "$bin\llvm-ar.exe" $dlltool }
    }
    $shim = "$tools\lld\ld.exe"
    if (-not (Test-Path $shim) -or (Get-Item "$root\tools\ld-shim.rs").LastWriteTime -gt (Get-Item $shim).LastWriteTime) {
        Invoke-Native rustc -O --edition 2024 "$root\tools\ld-shim.rs" -o $shim
    }
    $env:PEAR_LITE_RUST_LLD = "$bin\rust-lld.exe"
    $env:CARGO_ENCODED_RUSTFLAGS = @("-Cdlltool=$dlltool", "-Clink-arg=-B$tools\lld\") -join [char]0x1f
}

if ($Test) {
    Invoke-Native cargo test --manifest-path "$root\Cargo.toml"
    exit 0
}

Invoke-Native cargo build --release --manifest-path "$root\Cargo.toml"

New-Item -ItemType Directory -Force "$root\dist" | Out-Null
$exe = "$root\dist\PearLite.exe"
Copy-Item "$root\target\release\pear-lite.exe" $exe -Force

# Embed icon, version info and manifest (works with either toolchain, no rc.exe/windres needed).
if (-not ('PearLiteResources' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;

public static class PearLiteResources {
    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    static extern IntPtr BeginUpdateResource(string file, bool deleteExisting);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool UpdateResource(IntPtr h, IntPtr type, IntPtr name, ushort lang, byte[] data, uint size);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool EndUpdateResource(IntPtr h, bool discard);

    const ushort Lang = 1033;

    static void Put(IntPtr h, int type, int id, byte[] data) {
        if (!UpdateResource(h, (IntPtr)type, (IntPtr)id, Lang, data, (uint)data.Length))
            throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error());
    }

    public static void Apply(string exe, string ico, string manifest, int[] v, string description, string product) {
        IntPtr h = BeginUpdateResource(exe, false);
        if (h == IntPtr.Zero) throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error());

        // .ico file -> RT_ICON entries + RT_GROUP_ICON directory pointing at them.
        byte[] icon = File.ReadAllBytes(ico);
        int count = BitConverter.ToUInt16(icon, 4);
        var group = new MemoryStream();
        var g = new BinaryWriter(group);
        g.Write((ushort)0); g.Write((ushort)1); g.Write((ushort)count);
        for (int i = 0; i < count; i++) {
            int e = 6 + 16 * i;
            int size = BitConverter.ToInt32(icon, e + 8), offset = BitConverter.ToInt32(icon, e + 12);
            byte[] img = new byte[size];
            Array.Copy(icon, offset, img, 0, size);
            Put(h, 3, i + 1, img);
            g.Write(icon, e, 12);
            g.Write((ushort)(i + 1));
        }
        Put(h, 14, 1, group.ToArray());

        Put(h, 16, 1, VersionInfo(v, description, product));
        Put(h, 24, 1, File.ReadAllBytes(manifest));

        if (!EndUpdateResource(h, false)) throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error());
    }

    static void Pad(BinaryWriter w) { while (w.BaseStream.Position % 4 != 0) w.Write((byte)0); }

    static byte[] Block(string key, byte[] value, int valueLength, bool text, params byte[][] children) {
        var ms = new MemoryStream();
        var w = new BinaryWriter(ms);
        w.Write((ushort)0);
        w.Write((ushort)valueLength);
        w.Write((ushort)(text ? 1 : 0));
        w.Write(Encoding.Unicode.GetBytes(key + "\0"));
        Pad(w);
        if (value != null) w.Write(value);
        foreach (var c in children) { Pad(w); w.Write(c); }
        byte[] b = ms.ToArray();
        BitConverter.GetBytes((ushort)b.Length).CopyTo(b, 0);
        return b;
    }

    static byte[] Str(string key, string val) {
        return Block(key, Encoding.Unicode.GetBytes(val + "\0"), val.Length + 1, true);
    }

    static byte[] VersionInfo(int[] v, string description, string product) {
        string ver = v[0] + "." + v[1] + "." + v[2] + ".0";
        var f = new MemoryStream();
        var w = new BinaryWriter(f);
        uint ms = (uint)((v[0] << 16) | v[1]), ls = (uint)(v[2] << 16);
        foreach (uint x in new uint[] { 0xFEEF04BD, 0x00010000, ms, ls, ms, ls, 0x3F, 0, 0x40004, 1, 0, 0, 0 }) w.Write(x);
        byte[] table = Block("040904B0", null, 0, true,
            Str("CompanyName", "Pear Lite"),
            Str("FileDescription", description),
            Str("FileVersion", ver),
            Str("InternalName", "pear-lite"),
            Str("LegalCopyright", "MIT License"),
            Str("OriginalFilename", "PearLite.exe"),
            Str("ProductName", product),
            Str("ProductVersion", ver));
        byte[] strings = Block("StringFileInfo", null, 0, true, table);
        byte[] vars = Block("VarFileInfo", null, 0, true, Block("Translation", new byte[] { 0x09, 0x04, 0xB0, 0x04 }, 4, false));
        return Block("VS_VERSION_INFO", f.ToArray(), 52, false, strings, vars);
    }
}
'@
}
$version = ((Get-Content "$root\Cargo.toml") -match '^version\s*=' | Select-Object -First 1) -replace '.*"(.+)".*', '$1'
[int[]]$v = $version.Split('.')
[PearLiteResources]::Apply($exe, "$root\assets\app.ico", "$root\assets\app.manifest", $v, 'Pear Lite', 'Pear Lite')

$size = [math]::Round((Get-Item $exe).Length / 1MB, 2)
Write-Host "Built $exe ($size MB)" -ForegroundColor Green
if ($Run) { Start-Process $exe }
