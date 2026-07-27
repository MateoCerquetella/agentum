use std::fs::{File, OpenOptions};
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension as _, params};

pub(crate) struct Database {
    connection: Mutex<Connection>,
    _instance_lock: File,
}

pub(crate) struct DeviceBinding {
    pub connection_id: String,
    pub device_public_key: Vec<u8>,
    pub refresh_token_hash: String,
    pub credential_revision: i64,
    pub previous_refresh_token_hash: Option<String>,
    pub previous_credential_revision: Option<i64>,
    pub previous_valid_until_unix: Option<i64>,
}

impl Database {
    pub(crate) fn open(path: &Path) -> Result<Self, DatabaseError> {
        validate_database_location(path)?;
        let lock_path = lock_path(path)?;
        let instance_lock = secure_open(&lock_path)?;
        validate_private_file(&instance_lock)?;
        acquire_instance_lock(&instance_lock)?;
        let database_file = secure_open(path)?;
        validate_private_file(&database_file)?;
        drop(database_file);

        let connection = Connection::open(path).map_err(|_| DatabaseError::Unavailable)?;
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(|_| DatabaseError::Unavailable)?;
        connection
            .execute_batch(
                "PRAGMA foreign_keys = ON;
                 PRAGMA journal_mode = DELETE;
                 PRAGMA synchronous = FULL;
                 PRAGMA secure_delete = ON;
                 PRAGMA trusted_schema = OFF;
                 CREATE TABLE IF NOT EXISTS jira_device_bindings (
                   connection_id TEXT PRIMARY KEY NOT NULL,
                   device_public_key BLOB NOT NULL CHECK(length(device_public_key) = 32),
                   refresh_token_hash TEXT NOT NULL CHECK(length(refresh_token_hash) = 64),
                   credential_revision INTEGER NOT NULL CHECK(credential_revision > 0),
                   previous_refresh_token_hash TEXT CHECK(
                     previous_refresh_token_hash IS NULL OR length(previous_refresh_token_hash) = 64
                   ),
                   previous_credential_revision INTEGER CHECK(
                     previous_credential_revision IS NULL OR previous_credential_revision > 0
                   ),
                   previous_valid_until_unix INTEGER,
                   created_at_unix INTEGER NOT NULL,
                   updated_at_unix INTEGER NOT NULL,
                   CHECK (
                     (previous_refresh_token_hash IS NULL
                       AND previous_credential_revision IS NULL
                       AND previous_valid_until_unix IS NULL)
                     OR
                     (previous_refresh_token_hash IS NOT NULL
                       AND previous_credential_revision IS NOT NULL
                       AND previous_valid_until_unix IS NOT NULL)
                   )
                 ) STRICT;",
            )
            .map_err(|_| DatabaseError::Unavailable)?;
        Ok(Self {
            connection: Mutex::new(connection),
            _instance_lock: instance_lock,
        })
    }

    pub(crate) fn binding(
        &self,
        connection_id: &str,
    ) -> Result<Option<DeviceBinding>, DatabaseError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| DatabaseError::Unavailable)?;
        connection
            .query_row(
                "SELECT connection_id, device_public_key, refresh_token_hash, credential_revision,
                        previous_refresh_token_hash, previous_credential_revision,
                        previous_valid_until_unix
                 FROM jira_device_bindings
                 WHERE connection_id = ?1",
                [connection_id],
                |row| {
                    Ok(DeviceBinding {
                        connection_id: row.get(0)?,
                        device_public_key: row.get(1)?,
                        refresh_token_hash: row.get(2)?,
                        credential_revision: row.get(3)?,
                        previous_refresh_token_hash: row.get(4)?,
                        previous_credential_revision: row.get(5)?,
                        previous_valid_until_unix: row.get(6)?,
                    })
                },
            )
            .optional()
            .map_err(|_| DatabaseError::Unavailable)
    }

    pub(crate) fn replace_binding(
        &self,
        connection_id: &str,
        device_public_key: &[u8],
        refresh_token_hash: &str,
        now_unix: i64,
    ) -> Result<(), DatabaseError> {
        if device_public_key.len() != 32 || refresh_token_hash.len() != 64 {
            return Err(DatabaseError::Unavailable);
        }
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| DatabaseError::Unavailable)?;
        let transaction = connection
            .transaction()
            .map_err(|_| DatabaseError::Unavailable)?;
        transaction
            .execute(
                "INSERT INTO jira_device_bindings (
                   connection_id, device_public_key, refresh_token_hash,
                   credential_revision, previous_refresh_token_hash,
                   previous_credential_revision, previous_valid_until_unix,
                   created_at_unix, updated_at_unix
                 ) VALUES (?1, ?2, ?3, 1, NULL, NULL, NULL, ?4, ?4)
                 ON CONFLICT(connection_id) DO UPDATE SET
                   device_public_key = excluded.device_public_key,
                   refresh_token_hash = excluded.refresh_token_hash,
                   credential_revision = 1,
                   previous_refresh_token_hash = NULL,
                   previous_credential_revision = NULL,
                   previous_valid_until_unix = NULL,
                   updated_at_unix = excluded.updated_at_unix",
                params![
                    connection_id,
                    device_public_key,
                    refresh_token_hash,
                    now_unix
                ],
            )
            .map_err(|_| DatabaseError::Unavailable)?;
        transaction.commit().map_err(|_| DatabaseError::Unavailable)
    }

    pub(crate) fn replace_refresh_hash(
        &self,
        connection_id: &str,
        expected_revision: i64,
        expected_hash: &str,
        replacement_hash: &str,
        now_unix: i64,
        replay_valid_until_unix: i64,
    ) -> Result<bool, DatabaseError> {
        if expected_revision <= 0 || expected_hash.len() != 64 || replacement_hash.len() != 64 {
            return Err(DatabaseError::Unavailable);
        }
        let connection = self
            .connection
            .lock()
            .map_err(|_| DatabaseError::Unavailable)?;
        let changed = connection
            .execute(
                "UPDATE jira_device_bindings
                 SET refresh_token_hash = ?1,
                     credential_revision = credential_revision + 1,
                     previous_refresh_token_hash = refresh_token_hash,
                     previous_credential_revision = credential_revision,
                     previous_valid_until_unix = ?2,
                     updated_at_unix = ?3
                 WHERE connection_id = ?4
                   AND credential_revision = ?5
                   AND refresh_token_hash = ?6",
                params![
                    replacement_hash,
                    replay_valid_until_unix,
                    now_unix,
                    connection_id,
                    expected_revision,
                    expected_hash
                ],
            )
            .map_err(|_| DatabaseError::Unavailable)?;
        Ok(changed == 1)
    }

    pub(crate) fn recover_refresh_hash(
        &self,
        connection_id: &str,
        previous_revision: i64,
        previous_hash: &str,
        replacement_hash: &str,
        now_unix: i64,
    ) -> Result<bool, DatabaseError> {
        if previous_revision <= 0 || previous_hash.len() != 64 || replacement_hash.len() != 64 {
            return Err(DatabaseError::Unavailable);
        }
        let connection = self
            .connection
            .lock()
            .map_err(|_| DatabaseError::Unavailable)?;
        let changed = connection
            .execute(
                "UPDATE jira_device_bindings
                 SET refresh_token_hash = ?1,
                     updated_at_unix = ?2
                 WHERE connection_id = ?3
                   AND credential_revision = ?4 + 1
                   AND previous_credential_revision = ?4
                   AND previous_refresh_token_hash = ?5
                   AND previous_valid_until_unix > ?2",
                params![
                    replacement_hash,
                    now_unix,
                    connection_id,
                    previous_revision,
                    previous_hash
                ],
            )
            .map_err(|_| DatabaseError::Unavailable)?;
        Ok(changed == 1)
    }
}

fn validate_database_location(path: &Path) -> Result<(), DatabaseError> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
    {
        return Err(DatabaseError::UnsafePath);
    }
    let parent = path.parent().ok_or(DatabaseError::UnsafePath)?;
    validate_database_parent(parent)
}

#[cfg(not(windows))]
fn validate_database_parent(parent: &Path) -> Result<(), DatabaseError> {
    let canonical = parent
        .canonicalize()
        .map_err(|_| DatabaseError::UnsafePath)?;
    if canonical != parent {
        return Err(DatabaseError::UnsafePath);
    }
    let metadata = std::fs::symlink_metadata(parent).map_err(|_| DatabaseError::UnsafePath)?;
    if !metadata.is_dir() {
        return Err(DatabaseError::UnsafePath);
    }
    validate_private_parent(&metadata)
}

#[cfg(windows)]
fn validate_database_parent(parent: &Path) -> Result<(), DatabaseError> {
    use std::os::windows::fs::MetadataExt as _;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

    // A Windows absolute path necessarily begins with a Prefix component (for
    // example `C:` or a UNC share). Validate each component with no-follow
    // metadata instead of rejecting every valid prefixed path or comparing the
    // lexical path with Windows' `\\?\` canonical form.
    let mut current = PathBuf::new();
    for component in parent.components() {
        match component {
            Component::Prefix(prefix) if current.as_os_str().is_empty() => {
                current.push(prefix.as_os_str());
            }
            Component::RootDir | Component::Normal(_) => {
                current.push(component.as_os_str());
                let metadata =
                    std::fs::symlink_metadata(&current).map_err(|_| DatabaseError::UnsafePath)?;
                if !metadata.is_dir()
                    || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
                {
                    return Err(DatabaseError::UnsafePath);
                }
            }
            Component::Prefix(_) | Component::ParentDir | Component::CurDir => {
                return Err(DatabaseError::UnsafePath);
            }
        }
    }
    if current != parent {
        return Err(DatabaseError::UnsafePath);
    }
    Ok(())
}

fn lock_path(path: &Path) -> Result<PathBuf, DatabaseError> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(DatabaseError::UnsafePath)?;
    Ok(path.with_file_name(format!("{name}.lock")))
}

fn secure_open(path: &Path) -> Result<File, DatabaseError> {
    if let Ok(metadata) = std::fs::symlink_metadata(path)
        && (!metadata.file_type().is_file() || metadata.file_type().is_symlink())
    {
        return Err(DatabaseError::UnsafePath);
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options
            .mode(0o600)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt as _;
        use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;

        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    options.open(path).map_err(|_| DatabaseError::UnsafePath)
}

#[cfg(unix)]
fn validate_private_parent(metadata: &std::fs::Metadata) -> Result<(), DatabaseError> {
    use std::os::unix::fs::MetadataExt as _;

    if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o022 != 0 {
        return Err(DatabaseError::UnsafePath);
    }
    Ok(())
}

#[cfg(all(not(unix), not(windows)))]
fn validate_private_parent(_metadata: &std::fs::Metadata) -> Result<(), DatabaseError> {
    Ok(())
}

#[cfg(unix)]
fn validate_private_file(file: &File) -> Result<(), DatabaseError> {
    use std::os::unix::fs::MetadataExt as _;

    let metadata = file.metadata().map_err(|_| DatabaseError::UnsafePath)?;
    if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
        return Err(DatabaseError::UnsafePath);
    }
    Ok(())
}

#[cfg(all(not(unix), not(windows)))]
fn validate_private_file(_file: &File) -> Result<(), DatabaseError> {
    Ok(())
}

#[cfg(windows)]
fn validate_private_file(file: &File) -> Result<(), DatabaseError> {
    use std::os::windows::fs::MetadataExt as _;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

    let metadata = file.metadata().map_err(|_| DatabaseError::UnsafePath)?;
    if !metadata.is_file() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(DatabaseError::UnsafePath);
    }
    Ok(())
}

#[cfg(unix)]
fn acquire_instance_lock(file: &File) -> Result<(), DatabaseError> {
    use std::os::fd::AsRawFd as _;

    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result == 0 {
        Ok(())
    } else {
        Err(DatabaseError::AlreadyRunning)
    }
}

#[cfg(not(unix))]
fn acquire_instance_lock(_file: &File) -> Result<(), DatabaseError> {
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum DatabaseError {
    #[error("broker database path is unsafe")]
    UnsafePath,
    #[cfg(unix)]
    #[error("another broker process already owns this database")]
    AlreadyRunning,
    #[error("broker database is unavailable")]
    Unavailable,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_only_device_metadata_and_token_hashes() {
        let directory = tempfile::tempdir().unwrap();
        let database = Database::open(&directory.path().join("broker.sqlite3")).unwrap();
        let key = [7_u8; 32];
        let old_hash = "a".repeat(64);
        database
            .replace_binding("jira-123", &key, &old_hash, 10)
            .unwrap();
        let binding = database.binding("jira-123").unwrap().unwrap();
        assert_eq!(binding.connection_id, "jira-123");
        assert_eq!(binding.device_public_key, key);
        assert_eq!(binding.refresh_token_hash, old_hash);
        assert_eq!(binding.credential_revision, 1);

        let new_hash = "b".repeat(64);
        assert!(
            database
                .replace_refresh_hash("jira-123", 1, &old_hash, &new_hash, 20, 620)
                .unwrap()
        );
        assert!(
            !database
                .replace_refresh_hash("jira-123", 1, &old_hash, &new_hash, 21, 620)
                .unwrap()
        );
        let binding = database.binding("jira-123").unwrap().unwrap();
        assert_eq!(binding.refresh_token_hash, new_hash);
        assert_eq!(binding.credential_revision, 2);
        assert_eq!(
            binding.previous_refresh_token_hash.as_deref(),
            Some(old_hash.as_str())
        );
        assert_eq!(binding.previous_credential_revision, Some(1));
        assert!(
            database
                .recover_refresh_hash("jira-123", 1, &old_hash, &"c".repeat(64), 30)
                .unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_database() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("target.sqlite3");
        std::fs::write(&target, []).unwrap();
        let link = directory.path().join("link.sqlite3");
        symlink(&target, &link).unwrap();
        assert!(matches!(
            Database::open(&link),
            Err(DatabaseError::UnsafePath)
        ));
    }
}
