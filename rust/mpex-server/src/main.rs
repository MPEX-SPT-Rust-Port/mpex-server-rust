use std::path::{Path, PathBuf};

use netcorehost::nethost;
use netcorehost::pdcstring::PdCString;

/// The publish assembly name depends on the RID: `AssemblyName` is `SPT.Server`, overridden to
/// `SPT.Server.Linux` for linux-x64 publishes (SPTarkov.Server.csproj). Probe both.
const APP_DLL_NAMES: [&str; 2] = ["SPT.Server.dll", "SPT.Server.Linux.dll"];

fn find_app_dll(dir: &Path) -> Option<PathBuf> {
    APP_DLL_NAMES
        .iter()
        .map(|n| dir.join(n))
        .find(|p| p.is_file())
}

fn main() {
    match run() {
        Ok(code) => std::process::exit(code),
        Err(msg) => {
            eprintln!("mpex-server: {msg}");
            std::process::exit(1);
        }
    }
}

fn run() -> Result<i32, String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot locate own executable: {e}"))?;
    let exe_dir = exe.parent().ok_or("executable has no parent directory")?;
    let app_dll = find_app_dll(exe_dir).ok_or_else(|| {
        format!(
            "no server assembly ({}) beside the launcher in {}",
            APP_DLL_NAMES.join(" or "),
            exe_dir.display()
        )
    })?;
    let app_dll = PdCString::from_os_str(app_dll.as_os_str())
        .map_err(|e| format!("server assembly path is not a valid host string: {e}"))?;

    // Apphost-style discovery: finds the app-local hostfxr of a self-contained publish first,
    // then an installed runtime / DOTNET_ROOT. This is the whole runtime-acquisition story.
    let hostfxr = nethost::load_hostfxr_with_assembly_path(&app_dll).map_err(|e| {
        format!(
            "hostfxr not found (need the self-contained runtime files beside the launcher, \
             or an installed .NET runtime / DOTNET_ROOT): {e:?}"
        )
    })?;

    let args = std::env::args_os()
        .skip(1)
        .map(|a| PdCString::from_os_str(&a))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("argument is not a valid host string: {e}"))?;

    // Anchor: the linker drops an rlib the binary never references, taking all 34 #[no_mangle]
    // spt_* exports with it. Any path reference keeps them; this one is a call behind black_box so
    // that deleting it reads as a behaviour change rather than dead-code cleanup.
    // scripts/smoke-mpex-server.sh checks the launcher still exports them.
    std::hint::black_box(spt_native::ffi::spt_native_abi_version());

    let context = hostfxr
        .initialize_for_dotnet_command_line_with_args(&app_dll, args.iter().map(AsRef::as_ref))
        .map_err(|e| format!("runtime initialization failed: {e:?}"))?;

    Ok(context.run_app().value())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    #[test]
    fn finds_the_portable_name() {
        let dir = tempfile::tempdir().unwrap();
        File::create(dir.path().join("SPT.Server.dll")).unwrap();
        assert_eq!(
            find_app_dll(dir.path()),
            Some(dir.path().join("SPT.Server.dll"))
        );
    }

    #[test]
    fn finds_the_linux_rid_name() {
        let dir = tempfile::tempdir().unwrap();
        File::create(dir.path().join("SPT.Server.Linux.dll")).unwrap();
        assert_eq!(
            find_app_dll(dir.path()),
            Some(dir.path().join("SPT.Server.Linux.dll"))
        );
    }

    #[test]
    fn none_when_no_assembly_present() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(find_app_dll(dir.path()), None);
    }
}
