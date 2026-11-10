// Build-only helper for the windows-gnu toolchain (not part of the app).
//
// rustup's MinGW `gcc` always runs a program called `ld`. GNU ld mislinks the
// LLVM-style import libraries that the `windows` crates need, so build.ps1 puts
// this shim first on gcc's search path and it forwards to rust-lld instead.
//
// gcc passes arguments in GNU-quoted response files, while lld's MinGW driver on
// Windows always reads response files with Windows quoting (it would split paths
// containing spaces), so response files are re-encoded on the way through.
use std::ffi::OsString;
use std::process::{Command, exit};

/// libiberty `buildargv` rules: whitespace separates, backslash escapes, quotes group.
fn split_gnu(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_arg = false;
    let mut quote: Option<char> = None;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match (c, quote) {
            ('\\', _) => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
                in_arg = true;
            }
            (q, Some(open)) if q == open => quote = None,
            (c, Some(_)) => cur.push(c),
            ('"' | '\'', None) => {
                quote = Some(c);
                in_arg = true;
            }
            (c, None) if c.is_whitespace() => {
                if in_arg {
                    out.push(std::mem::take(&mut cur));
                    in_arg = false;
                }
            }
            (c, None) => {
                cur.push(c);
                in_arg = true;
            }
        }
    }
    if in_arg {
        out.push(cur);
    }
    out
}

/// MSVC command-line quoting, as understood by LLVM's `TokenizeWindowsCommandLine`.
fn quote_windows(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        return arg.to_string();
    }
    let mut out = String::from("\"");
    let mut backslashes = 0;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                out.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            c => {
                out.extend(std::iter::repeat_n('\\', backslashes));
                out.push(c);
                backslashes = 0;
            }
        }
    }
    out.extend(std::iter::repeat_n('\\', backslashes * 2));
    out.push('"');
    out
}

fn main() {
    let lld = std::env::var_os("PEAR_LITE_RUST_LLD").expect("PEAR_LITE_RUST_LLD is not set");
    let mut args: Vec<OsString> = vec!["-flavor".into(), "gnu".into()];
    let mut temp_files = Vec::new();
    for (i, arg) in std::env::args_os().skip(1).enumerate() {
        let rsp = arg.to_str().and_then(|a| a.strip_prefix('@')).map(str::to_owned);
        match rsp.and_then(|path| std::fs::read_to_string(path).ok()) {
            Some(text) => {
                let body: Vec<String> = split_gnu(&text).iter().map(|a| quote_windows(a)).collect();
                let tmp = std::env::temp_dir().join(format!("ld-shim-{}-{i}.rsp", std::process::id()));
                std::fs::write(&tmp, body.join("\n")).expect("write response file");
                args.push(format!("@{}", tmp.display()).into());
                temp_files.push(tmp);
            }
            None => args.push(arg),
        }
    }
    let status = Command::new(lld).args(&args).status();
    for f in temp_files {
        let _ = std::fs::remove_file(f);
    }
    match status {
        Ok(s) => exit(s.code().unwrap_or(1)),
        Err(e) => {
            eprintln!("ld-shim: cannot run rust-lld: {e}");
            exit(1)
        }
    }
}
