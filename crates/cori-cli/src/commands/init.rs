//! `cori init --local`.

use anyhow::{Context, Result};

use crate::{paths, runtime};

pub fn run(local: bool) -> Result<()> {
    if !local {
        // v1 only supports `--local`. Surface a clear error instead of doing
        // something surprising. Future phases may add cloud-attached modes.
        anyhow::bail!("cori init currently requires `--local`. Re-run as `cori init --local`.");
    }

    let home = paths::home()?;
    let runbooks = paths::runbooks_dir()?;
    let state = paths::state_dir()?;
    let logs = paths::logs_dir()?;
    let db = paths::registry_db()?;
    let runtime_root = paths::runtime_dir()?;

    let created_home = create_if_missing(&home, "directory")?;
    let created_runbooks = create_if_missing(&runbooks, "directory")?;
    let created_state = create_if_missing(&state, "directory")?;
    let created_logs = create_if_missing(&logs, "directory")?;
    let runtime_existed = runtime_root.exists();

    // Touching the DB also runs the schema.
    let db_existed = db.exists();
    let _ = crate::registry::open().context("initialising SQLite registry")?;

    // Install the Deno runner + bundled SDK. Always run — `install_at`
    // detects unchanged files and no-ops them, but a binary upgrade should
    // refresh stale copies.
    let runtime_report =
        runtime::install().context("installing Deno runtime files under `runtime/`")?;

    println!("✓ Cori home: {}", home.display());
    print_status("  runbooks/", created_runbooks);
    print_status("  state/   ", created_state);
    print_status("  logs/    ", created_logs);
    print_status(
        "  registry.db",
        if db_existed {
            Status::Existed
        } else {
            Status::Created
        },
    );
    let runtime_status = if !runtime_existed {
        Status::Created
    } else if runtime_report.any_change() {
        Status::Updated
    } else {
        Status::Existed
    };
    print_status("  runtime/  (Deno runner + SDK)", runtime_status);

    if !created_home.is_created()
        && !created_runbooks.is_created()
        && db_existed
        && !runtime_report.any_change()
    {
        println!("Already initialised. Nothing to do.");
    }
    Ok(())
}

#[derive(Copy, Clone)]
enum Status {
    Created,
    Existed,
    Updated,
}
impl Status {
    fn is_created(self) -> bool {
        matches!(self, Status::Created)
    }
}

fn create_if_missing(path: &std::path::Path, kind: &str) -> Result<Status> {
    if path.exists() {
        Ok(Status::Existed)
    } else {
        std::fs::create_dir_all(path)
            .with_context(|| format!("creating {kind} `{}`", path.display()))?;
        Ok(Status::Created)
    }
}

fn print_status(label: &str, status: Status) {
    match status {
        Status::Created => println!("  + {label}  (created)"),
        Status::Existed => println!("  · {label}  (already present)"),
        Status::Updated => println!("  ~ {label}  (refreshed)"),
    }
}
