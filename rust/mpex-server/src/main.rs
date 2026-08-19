use std::path::{Path, PathBuf};

/// The publish assembly name depends on the RID: `AssemblyName` is `SPT.Server`, overridden to
/// `SPT.Server.Linux` for linux-x64 publishes (SPTarkov.Server.csproj). Probe both.
const APP_DLL_NAMES: [&str; 2] = ["SPT.Server.dll", "SPT.Server.Linux.dll"];

// Dead in the bin target until Task 2's `run()` calls it; `expect` forces removal then.
#[cfg_attr(not(test), expect(dead_code))]
fn find_app_dll(dir: &Path) -> Option<PathBuf> {
    APP_DLL_NAMES
        .iter()
        .map(|n| dir.join(n))
        .find(|p| p.is_file())
}

fn main() {
    unimplemented!("Task 2");
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
