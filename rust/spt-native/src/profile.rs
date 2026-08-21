//! Disk half of `Libraries/SPTarkov.Server.Core/Servers/SaveServer.cs` (`LoadAsync:40`,
//! `LoadProfileAsync:188`, `SaveProfileAsync:253`, `RemoveProfile:298`) plus the atomic-write
//! protocol of `Utils/FileUtil.cs::WriteFileAsync:113`. The live `SptProfile` graph, the MD5
//! dirty-check, migration, and `BackupService` all stay C# — profile bytes are opaque here,
//! written and read verbatim. Stateless: the profiles directory arrives in every request
//! (`db/load.rs::LoadRequest::dir` precedent), relative to the process CWD.

use std::fs;
use std::io::Write;
use std::path::Path;

use serde::Deserialize;
use serde_json::value::RawValue;

#[derive(Debug)]
pub enum ProfileError {
    /// Malformed request (wrong schema, bad id): the caller's bug.
    BadArgs(String),
    /// The disk said no: message names the path and the OS error.
    Io(String),
}

fn gate_schema(schema: u32) -> Result<(), ProfileError> {
    if schema == 1 {
        Ok(())
    } else {
        Err(ProfileError::BadArgs(format!(
            "unsupported schema {schema}, expected 1"
        )))
    }
}

/// Mirrors `MongoId.IsValidMongoId` (`Extensions/MongoIdExtensions.cs:52-68`: length exactly 24,
/// every char in `0-9a-fA-F`). Doubles as the path-traversal guard: an id that passes cannot
/// contain a separator, a dot, or a parent reference.
fn gate_id(id: &str) -> Result<(), ProfileError> {
    if id.len() == 24 && id.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(ProfileError::BadArgs(format!("not a profile id: {id:?}")))
    }
}

fn io_err(path: &Path, error: std::io::Error) -> ProfileError {
    ProfileError::Io(format!("{}: {error}", path.display()))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListRequest {
    pub schema: u32,
    /// e.g. `"user/profiles/"` — SaveServer's `profileFilepath`, relative to CWD.
    pub dir: String,
}

/// Every file's name in the directory, non-recursive, sorted. Creates the directory when
/// missing (replaces SaveServer's DirectoryExists/CreateDirectory pair). All filtering
/// (`.json` extension, MongoId stems) stays C#-side, verbatim — zero filter-parity risk.
/// Deviation: sorted; `Directory.GetFiles` order was filesystem-dependent.
///
/// `fs::metadata` and not `entry.metadata()`/`entry.file_type()`: only the free function
/// follows symlinks — both `DirEntry` methods report the link itself (`lstat` on Unix) and
/// would call a symlink-to-a-profile neither file nor directory. `Directory.GetFiles` returns
/// symlinks to files and omits symlinks to directories, which is exactly what
/// following-then-`is_file()` reproduces.
///
/// Only `NotFound` is swallowed, and every other `stat` failure is raised, because this is
/// where `stat`-per-entry stops matching `Directory.GetFiles`. `readdir` needs only read
/// (`+r`) on the directory; `stat`ping a child needs search (`+x`). .NET's Unix enumerator
/// answers file-vs-directory from `getdents64`'s `d_type` and never `stat`s a `DT_REG` entry,
/// so on a `user/profiles/` that has lost `+x` it still lists every profile (measured on .NET
/// 10: `Directory.GetFiles` returns the file while `File.Exists` on that same child is
/// `false`). Here every entry's `stat` fails `EACCES` instead. Swallowing that would report an
/// empty directory, and `LoadAsync` would come up with zero profiles and invite the player to
/// create a new one beside intact files — the worst presentation of a `chmod`-recoverable
/// condition. Failing the call names the directory and the errno.
pub fn list(req: ListRequest) -> Result<Vec<String>, ProfileError> {
    gate_schema(req.schema)?;
    let dir = Path::new(&req.dir);
    fs::create_dir_all(dir).map_err(|e| io_err(dir, e))?;
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).map_err(|e| io_err(dir, e))? {
        let entry = entry.map_err(|e| io_err(dir, e))?;
        let is_file = match fs::metadata(entry.path()) {
            Ok(metadata) => metadata.is_file(),
            // A dangling symlink: `Directory.GetFiles` lists it, the C# extension and stem
            // filters drop it, and it has no bytes to load. Skipping matches the outcome.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(io_err(&entry.path(), error)),
        };
        if is_file {
            files.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    files.sort();
    Ok(files)
}

pub fn encode_list(files: Vec<String>) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({ "files": files }))
        .expect("a string array always serializes")
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadRequest {
    pub schema: u32,
    pub dir: String,
    pub id: String,
}

pub struct Loaded {
    pub found: bool,
    pub bytes: Vec<u8>,
}

/// The file's bytes, or `found: false` when it does not exist — the same branch
/// `fileUtil.FileExists` fed in the C# body. Corrupt JSON is NOT detected here; C# discovers
/// it on deserialize and runs its unchanged backup-recovery arm.
///
/// A leading UTF-8 BOM is dropped, and that is load-bearing parity, not tidiness: this
/// replaces `JsonSerializer.DeserializeAsync<T>(Stream)` (`JsonUtil.cs:109-111`), which
/// consumes a BOM, with the `ReadOnlySpan<byte>` overload (`JsonUtil.cs:71`), whose
/// `Utf8JsonReader` treats one as an invalid start of a value. Without the strip, a
/// hand-edited BOM'd profile that loads today would throw `JsonException`, take the
/// corrupt-recovery arm, and roll the player back to a backup. Same guard as
/// `db/load.rs:146` applies to every eager database file.
pub fn load(req: LoadRequest) -> Result<Loaded, ProfileError> {
    gate_schema(req.schema)?;
    gate_id(&req.id)?;
    let path = Path::new(&req.dir).join(format!("{}.json", req.id));
    match fs::read(&path) {
        Ok(bytes) => Ok(Loaded {
            found: true,
            bytes: crate::db::load::strip_bom(bytes),
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Loaded {
            found: false,
            bytes: Vec::new(),
        }),
        Err(e) => Err(io_err(&path, e)),
    }
}

/// `[u32-LE header length][header JSON][file bytes]` — the `spt_db_load` frame layout, so
/// profile JSON crosses as bytes, never as an escaped JSON string.
///
/// Keep the `with_capacity` exact. `write_buffer` (`ffi.rs:214-221`) hands the buffer over with
/// `into_boxed_slice`, which reallocates and copies the whole frame when `len != capacity` —
/// filling an exactly-sized `Vec` makes the handoff free. Reading the file straight onto a
/// pre-headered buffer with `read_to_end` trades this copy for that one, and loses the exact
/// length; it is not an optimisation.
pub fn encode_load_frame(loaded: Loaded) -> Vec<u8> {
    let header: &[u8] = if loaded.found {
        br#"{"found":true}"#
    } else {
        br#"{"found":false}"#
    };
    let mut out = Vec::with_capacity(4 + header.len() + loaded.bytes.len());
    out.extend_from_slice(&(header.len() as u32).to_le_bytes());
    out.extend_from_slice(header);
    out.extend_from_slice(&loaded.bytes);
    out
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveRequest {
    pub schema: u32,
    pub dir: String,
    pub id: String,
    /// The serialized profile exactly as `jsonUtil.Serialize` produced it. `RawValue` keeps
    /// the text verbatim — indentation included — so the file is byte-identical to today's.
    pub profile: Box<RawValue>,
}

/// `FileUtil.WriteFileAsync` parity (`Utils/FileUtil.cs:113`): same `{id}.json.bak` temp
/// name, data fsync, rename over the live file, delete the temp on failure.
///
/// The `File` is deliberately confined to the first closure so it drops — and closes — before
/// `fs::rename` runs. This is load-bearing on Windows, where `fs::rename` is `MoveFileExW`
/// with `MOVEFILE_REPLACE_EXISTING` and fails with a sharing violation while the source
/// handle is open without `FILE_SHARE_DELETE`. Do not hoist the handle out of the chain; it
/// would still pass every test on Linux, which is the only platform this repo runs today
/// (`RUST-ROADMAP.md:974` — `mpex-server.exe` ships but has never been executed).
pub fn save(req: SaveRequest) -> Result<(), ProfileError> {
    gate_schema(req.schema)?;
    gate_id(&req.id)?;
    let dir = Path::new(&req.dir);
    fs::create_dir_all(dir).map_err(|e| io_err(dir, e))?;
    let live = dir.join(format!("{}.json", req.id));
    let tmp = dir.join(format!("{}.json.bak", req.id));
    let written = fs::File::create(&tmp)
        .and_then(|mut file| {
            file.write_all(req.profile.get().as_bytes())?;
            file.sync_all()
        })
        .and_then(|_| fs::rename(&tmp, &live));
    if let Err(error) = written {
        let _ = fs::remove_file(&tmp);
        return Err(io_err(&live, error));
    }
    Ok(())
    // ponytail: data fsync only, no directory fsync — matches FileUtil.WriteFileAsync
    // exactly; add a dir sync if crash reports ever show a vanished rename.
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteRequest {
    pub schema: u32,
    pub dir: String,
    pub id: String,
}

/// `true` = the file existed and was removed; `false` = it was not there (C# logs that,
/// exactly as `fileUtil.DeleteFile`'s false return does today). Real failures are `Io`.
pub fn delete(req: DeleteRequest) -> Result<bool, ProfileError> {
    gate_schema(req.schema)?;
    gate_id(&req.id)?;
    let path = Path::new(&req.dir).join(format!("{}.json", req.id));
    match fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(io_err(&path, e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    const ID: &str = "6889d9d1f8ee8ab88c0b8e11";

    fn dir_arg(dir: &TempDir) -> String {
        dir.path()
            .to_str()
            .expect("temp paths are utf-8")
            .to_owned()
    }

    /// Built through `from_str` so `profile` lands as the exact bytes a caller framed, the way
    /// the FFI export will deserialize it.
    fn save_request(dir: &TempDir, id: &str, profile: &str) -> SaveRequest {
        let dir = dir_arg(dir);
        serde_json::from_str(&format!(
            r#"{{"schema":1,"dir":{dir:?},"id":{id:?},"profile":{profile}}}"#
        ))
        .expect("request parses")
    }

    fn load_request(dir: &TempDir, id: &str) -> LoadRequest {
        LoadRequest {
            schema: 1,
            dir: dir_arg(dir),
            id: id.to_owned(),
        }
    }

    fn delete_request(dir: &TempDir, id: &str) -> DeleteRequest {
        DeleteRequest {
            schema: 1,
            dir: dir_arg(dir),
            id: id.to_owned(),
        }
    }

    fn live(dir: &TempDir) -> PathBuf {
        dir.path().join(format!("{ID}.json"))
    }

    fn temp(dir: &TempDir) -> PathBuf {
        dir.path().join(format!("{ID}.json.bak"))
    }

    const ODD: &str = "{\n  \"a\": 1,\r\n\t\"b\":[ ]\n}";

    #[test]
    fn save_round_trips_bytes_verbatim() {
        let dir = TempDir::new().expect("temp dir");
        save(save_request(&dir, ID, ODD)).expect("save succeeds");

        assert_eq!(fs::read(live(&dir)).expect("live file"), ODD.as_bytes());
    }

    #[test]
    fn save_leaves_no_temp_file() {
        let dir = TempDir::new().expect("temp dir");
        save(save_request(&dir, ID, "{}")).expect("save succeeds");

        assert!(!temp(&dir).exists());
    }

    #[test]
    fn save_creates_the_directory() {
        let outer = TempDir::new().expect("temp dir");
        let nested = outer.path().join("nested");
        let dir = nested.to_str().expect("temp paths are utf-8").to_owned();
        let req: SaveRequest = serde_json::from_str(&format!(
            r#"{{"schema":1,"dir":{dir:?},"id":"{ID}","profile":{{}}}}"#
        ))
        .expect("request parses");

        save(req).expect("save succeeds");

        assert_eq!(
            fs::read(nested.join(format!("{ID}.json"))).expect("live file"),
            b"{}"
        );
    }

    /// Pins the temp *name*: `BackupService`'s copy loop is unfiltered
    /// (`BackupService.cs:121`), so `{id}.json.bak` is part of the coexistence contract.
    /// Deliberately does not assert "temp gone" — the cleanup `remove_file` is a no-op against
    /// a directory and is intentionally `let _ =`.
    #[test]
    fn save_failure_at_the_temp_pins_the_temp_name() {
        let dir = TempDir::new().expect("temp dir");
        fs::create_dir(temp(&dir)).expect("temp name taken by a directory");

        let error = save(save_request(&dir, ID, "{}")).expect_err("create fails");

        assert!(matches!(error, ProfileError::Io(_)), "{error:?}");
        assert!(!live(&dir).exists());
    }

    #[test]
    fn save_failure_at_the_rename_cleans_the_temp() {
        let dir = TempDir::new().expect("temp dir");
        fs::create_dir(live(&dir)).expect("live name taken by a directory");

        let error = save(save_request(&dir, ID, "{}")).expect_err("rename fails");

        assert!(matches!(error, ProfileError::Io(_)), "{error:?}");
        assert!(!temp(&dir).exists());
        assert!(live(&dir).is_dir());
    }

    #[test]
    fn load_missing_is_found_false() {
        let dir = TempDir::new().expect("temp dir");

        let loaded = load(load_request(&dir, ID)).expect("load succeeds");

        assert!(!loaded.found);
        assert!(loaded.bytes.is_empty());
    }

    #[test]
    fn load_returns_bytes_verbatim() {
        let dir = TempDir::new().expect("temp dir");
        fs::write(live(&dir), b"not even json \x00\xFF").expect("seed");

        let loaded = load(load_request(&dir, ID)).expect("load succeeds");

        assert!(loaded.found);
        assert_eq!(loaded.bytes, b"not even json \x00\xFF");
    }

    /// The Global Constraint's parity pin: today's `FileStream` path skips a BOM, the span
    /// overload C# switches to would throw and trigger a backup rollback.
    #[test]
    fn load_strips_a_utf8_bom() {
        let dir = TempDir::new().expect("temp dir");
        fs::write(live(&dir), b"\xEF\xBB\xBF{\"a\":1}").expect("seed");

        let loaded = load(load_request(&dir, ID)).expect("load succeeds");

        assert_eq!(loaded.bytes, b"{\"a\":1}");
    }

    #[test]
    fn load_keeps_an_interior_bom_sequence() {
        let dir = TempDir::new().expect("temp dir");
        fs::write(live(&dir), b"{\"a\":\"\xEF\xBB\xBF\"}").expect("seed");

        let loaded = load(load_request(&dir, ID)).expect("load succeeds");

        assert_eq!(loaded.bytes, b"{\"a\":\"\xEF\xBB\xBF\"}");
    }

    #[test]
    fn list_names_files_skips_dirs_and_sorts() {
        let dir = TempDir::new().expect("temp dir");
        fs::write(dir.path().join("b.json"), b"{}").expect("seed");
        fs::write(dir.path().join("a.json.bak"), b"{}").expect("seed");
        fs::create_dir(dir.path().join("backups")).expect("seed");
        fs::write(dir.path().join("backups").join("c.json"), b"{}").expect("seed");

        let files = list(ListRequest {
            schema: 1,
            dir: dir_arg(&dir),
        })
        .expect("list succeeds");

        assert_eq!(files, vec!["a.json.bak".to_owned(), "b.json".to_owned()]);
    }

    /// The `Directory.GetFiles` split: a symlink to a file is listed, a symlink to a directory
    /// is not.
    #[cfg(unix)]
    #[test]
    fn list_follows_symlinks_like_getfiles() {
        let dir = TempDir::new().expect("temp dir");
        fs::write(dir.path().join("real.json"), b"{}").expect("seed");
        fs::create_dir(dir.path().join("backups")).expect("seed");
        std::os::unix::fs::symlink(dir.path().join("real.json"), dir.path().join("link.json"))
            .expect("file symlink");
        std::os::unix::fs::symlink(dir.path().join("backups"), dir.path().join("dirlink"))
            .expect("dir symlink");

        let files = list(ListRequest {
            schema: 1,
            dir: dir_arg(&dir),
        })
        .expect("list succeeds");

        assert!(files.contains(&"link.json".to_owned()), "{files:?}");
        assert!(!files.contains(&"dirlink".to_owned()), "{files:?}");
    }

    /// The one `stat` failure that stays silent: a dangling link has no bytes to load and the
    /// C# filters would drop it anyway, so skipping it reaches `Directory.GetFiles`' outcome.
    #[cfg(unix)]
    #[test]
    fn list_skips_a_dangling_symlink() {
        let dir = TempDir::new().expect("temp dir");
        fs::write(dir.path().join("real.json"), b"{}").expect("seed");
        std::os::unix::fs::symlink(dir.path().join("gone.json"), dir.path().join("dead.json"))
            .expect("dangling symlink");

        let files = list(ListRequest {
            schema: 1,
            dir: dir_arg(&dir),
        })
        .expect("a dangling link is not an error");

        assert_eq!(files, vec!["real.json".to_owned()]);
    }

    /// A profiles directory that has lost `+x` still `readdir`s but denies every child `stat`.
    /// `Directory.GetFiles` listed the profiles here (it reads `d_type` and never `stat`s a
    /// regular file); reporting an empty directory instead would have `LoadAsync` come up with
    /// zero profiles and offer to create a new one beside them. It has to fail loudly.
    #[cfg(unix)]
    #[test]
    fn list_raises_an_unreadable_entry() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().expect("temp dir");
        fs::write(dir.path().join("real.json"), b"{}").expect("seed");
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o444)).expect("drop +x");

        // Root bypasses the search bit, so the arm under test is unreachable there.
        let denied = fs::metadata(dir.path().join("real.json")).is_err();
        let result = list(ListRequest {
            schema: 1,
            dir: dir_arg(&dir),
        });

        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).expect("restore +x");
        if denied {
            assert!(
                matches!(result, Err(ProfileError::Io(_))),
                "an EACCES entry was swallowed: {result:?}"
            );
        }
    }

    #[test]
    fn list_creates_missing_dir() {
        let outer = TempDir::new().expect("temp dir");
        let nested = outer.path().join("nested");

        let files = list(ListRequest {
            schema: 1,
            dir: nested.to_str().expect("temp paths are utf-8").to_owned(),
        })
        .expect("list succeeds");

        assert!(files.is_empty());
        assert!(nested.is_dir());
    }

    #[test]
    fn delete_true_then_false() {
        let dir = TempDir::new().expect("temp dir");
        fs::write(live(&dir), b"{}").expect("seed");

        assert!(delete(delete_request(&dir, ID)).expect("delete succeeds"));
        assert!(!delete(delete_request(&dir, ID)).expect("delete succeeds"));
    }

    /// `MongoId.IsValidMongoId` is case-insensitive, so a mixed-case 24-char id is valid;
    /// the rejected ids are the traversal attempt, a 23-char id, and the empty string.
    #[test]
    fn ids_are_gated() {
        let dir = TempDir::new().expect("temp dir");

        for id in ["../evil", "6889D9d1f8ee8ab88c0b8e1", ""] {
            let loaded = load(load_request(&dir, id));
            assert!(
                matches!(loaded, Err(ProfileError::BadArgs(_))),
                "load {id:?}"
            );
            let saved = save(save_request(&dir, id, "{}"));
            assert!(
                matches!(saved, Err(ProfileError::BadArgs(_))),
                "save {id:?}"
            );
            let deleted = delete(delete_request(&dir, id));
            assert!(
                matches!(deleted, Err(ProfileError::BadArgs(_))),
                "delete {id:?}"
            );
        }

        let mixed = "6889D9d1F8Ee8ab88c0b8e11";
        assert!(load(load_request(&dir, mixed)).is_ok());
        assert!(save(save_request(&dir, mixed, "{}")).is_ok());
        assert!(delete(delete_request(&dir, mixed)).is_ok());
    }

    #[test]
    fn frame_layout() {
        let frame = encode_load_frame(Loaded {
            found: true,
            bytes: b"xy".to_vec(),
        });

        let mut expected = 14u32.to_le_bytes().to_vec();
        expected.extend_from_slice(br#"{"found":true}"#);
        expected.extend_from_slice(b"xy");
        assert_eq!(frame, expected);
    }
}
