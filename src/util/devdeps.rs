use anyhow::Context;
use std::{
    fs,
    path::{Path, PathBuf},
    process,
    time::{SystemTime, UNIX_EPOCH},
};
use toml_edit::{DocumentMut, Item, Key, Table};

pub(crate) const DEV_DEPENDENCY_RESTORE_FILE: &str = "removed-dev-dependencies.toml";
const MANIFEST_PATH_FILE: &str = "manifest-path";
const ORIGINAL_MANIFEST_FILE: &str = "original-manifest.toml";

#[derive(Debug)]
struct RemovedTomlEntry {
    key: Key,
    item: Item,
}

#[derive(Debug)]
struct RemovedDevDependencySection {
    target: Option<String>,
    entry: RemovedTomlEntry,
}

#[derive(Debug, Default)]
struct DevDependencySections {
    removed: Vec<RemovedDevDependencySection>,
}

#[derive(Debug)]
pub(crate) struct DevDependencyRestore {
    manifest_path: PathBuf,
    record_dir: Option<PathBuf>,
}

impl DevDependencyRestore {
    pub(crate) fn is_empty(&self) -> bool {
        self.record_dir.is_none()
    }

    /// Remove dev-dependency sections from a manifest and write a target-dir
    /// restore record that can merge only those sections back later.
    pub(crate) fn strip_manifest(manifest_path: &Path, record_dir: &Path) -> anyhow::Result<Self> {
        let manifest = fs::read_to_string(manifest_path)
            .with_context(|| format!("Could not read manifest at {}", manifest_path.display()))?;
        let mut document = manifest
            .parse::<DocumentMut>()
            .with_context(|| format!("Could not parse manifest at {}", manifest_path.display()))?;
        let removed = collect_dev_dependency_sections(&mut document);

        if removed.is_empty() {
            return Ok(Self {
                manifest_path: manifest_path.to_path_buf(),
                record_dir: None,
            });
        }

        removed.write_restore_record(record_dir, manifest_path, &manifest)?;
        if let Err(error) = write_text_replace(manifest_path, &document.to_string()) {
            return Err(error).with_context(|| {
                format!(
                    "Could not write manifest at {}; dev-dependency restore record remains at {}",
                    manifest_path.display(),
                    record_dir.display()
                )
            });
        }

        Ok(Self {
            manifest_path: manifest_path.to_path_buf(),
            record_dir: Some(record_dir.to_path_buf()),
        })
    }

    /// Restore only the dev-dependency entries recorded under `target` by
    /// `DevDependencyRestore::strip_manifest`.
    ///
    /// If the manifest is unchanged since stripping, restore the original
    /// bytes to preserve its formatting exactly. Otherwise, reparse the
    /// current manifest and merge only the removed sections back in so
    /// unrelated dirty-state edits such as version bumps are preserved.
    pub(crate) fn restore(self) -> anyhow::Result<()> {
        let Some(record_dir) = self.record_dir else {
            return Ok(());
        };

        let original_file = record_dir.join(ORIGINAL_MANIFEST_FILE);
        let original = fs::read_to_string(&original_file).with_context(|| {
            format!(
                "Could not read original manifest restore file at {}",
                original_file.display()
            )
        })?;
        let mut stripped_original = original.parse::<DocumentMut>().with_context(|| {
            format!(
                "Could not parse original manifest restore file at {}",
                original_file.display()
            )
        })?;
        collect_dev_dependency_sections(&mut stripped_original);
        let current = fs::read_to_string(&self.manifest_path).with_context(|| {
            format!(
                "Could not read manifest at {} while restoring dev-dependencies",
                self.manifest_path.display()
            )
        })?;
        if current == stripped_original.to_string() {
            write_text_replace(&self.manifest_path, &original).with_context(|| {
                format!(
                    "Could not write manifest at {} while restoring dev-dependencies",
                    self.manifest_path.display()
                )
            })?;
            let _ = fs::remove_dir_all(&record_dir);
            return Ok(());
        }

        let restore_file = record_dir.join(DEV_DEPENDENCY_RESTORE_FILE);
        let restore = fs::read_to_string(&restore_file).with_context(|| {
            format!(
                "Could not read dev-dependency restore file at {}",
                restore_file.display()
            )
        })?;
        let mut restore_document = restore.parse::<DocumentMut>().with_context(|| {
            format!(
                "Could not parse dev-dependency restore file at {}",
                restore_file.display()
            )
        })?;
        let removed = collect_dev_dependency_sections(&mut restore_document);
        removed.write_to_manifest(&self.manifest_path)?;

        let _ = fs::remove_dir_all(&record_dir);
        Ok(())
    }
}

#[derive(Debug)]
pub(crate) struct DevDependencyRestoreSession {
    root: PathBuf,
    restores: Vec<DevDependencyRestore>,
}

impl DevDependencyRestoreSession {
    pub(crate) fn new(target_dir: &Path) -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let root = target_dir
            .join("cargo-dragons")
            .join("dev-dependency-restore")
            .join(format!("{}-{timestamp}", process::id()));

        Self {
            root,
            restores: Vec::new(),
        }
    }

    pub(crate) fn strip_manifest(
        &mut self,
        label: impl AsRef<str>,
        manifest_path: &Path,
    ) -> anyhow::Result<()> {
        let record_dir = self.root.join(format!(
            "{}-{}",
            self.restores.len(),
            safe_path_segment(label.as_ref())
        ));

        match DevDependencyRestore::strip_manifest(manifest_path, &record_dir) {
            Ok(restore) => {
                self.restores.push(restore);
                Ok(())
            }
            Err(error) => match self.restore_written() {
                Ok(()) => Err(error),
                Err(restore_error) => Err(error.context(format!(
                    "Failed to restore already-stripped manifests after failing to strip {}: {restore_error:#}",
                    manifest_path.display()
                ))),
            },
        }
    }

    pub(crate) fn restore_all(mut self) -> anyhow::Result<()> {
        let result = self.restore_written();
        if result.is_ok() {
            let _ = fs::remove_dir_all(&self.root);
        }
        result
    }

    fn restore_written(&mut self) -> anyhow::Result<()> {
        let restores = Vec::from_iter(
            std::mem::take(&mut self.restores)
                .into_iter()
                .filter(|restore| !restore.is_empty()),
        );
        if restores.is_empty() {
            return Ok(());
        }

        let mut errors = Vec::new();
        for restore in restores {
            if let Err(error) = restore.restore() {
                errors.push(format!("{error:#}"));
            }
        }

        if !errors.is_empty() {
            anyhow::bail!(
                "Failed to restore one or more dev-dependency restore records:\n{}",
                errors.join("\n")
            );
        }

        Ok(())
    }
}

fn safe_path_segment(label: &str) -> String {
    let safe = String::from_iter(label.chars().map(|ch| match ch {
        'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' => ch,
        _ => '_',
    }));

    if safe.is_empty() {
        "manifest".to_owned()
    } else {
        safe
    }
}

fn remove_dev_dependency_entries(table: &mut Table) -> Vec<RemovedTomlEntry> {
    Vec::from_iter(
        ["dev-dependencies", "dev_dependencies"]
            .into_iter()
            .filter_map(|key| {
                table
                    .remove_entry(key)
                    .map(|(key, item)| RemovedTomlEntry { key, item })
            }),
    )
}

fn collect_dev_dependency_sections(document: &mut DocumentMut) -> DevDependencySections {
    let mut removed = Vec::new();
    let root = document.as_table_mut();
    removed.extend(
        remove_dev_dependency_entries(root)
            .into_iter()
            .map(|entry| RemovedDevDependencySection {
                target: None,
                entry,
            }),
    );

    if let Some(targets) = root.get_mut("target").and_then(Item::as_table_mut) {
        let target_names = Vec::from_iter(targets.iter().map(|(name, _)| name.to_owned()));
        for target_name in target_names {
            if let Some(target) = targets.get_mut(&target_name).and_then(Item::as_table_mut) {
                removed.extend(
                    remove_dev_dependency_entries(target)
                        .into_iter()
                        .map(|entry| RemovedDevDependencySection {
                            target: Some(target_name.clone()),
                            entry,
                        }),
                );
            }
        }
    }

    DevDependencySections { removed }
}

impl DevDependencySections {
    fn is_empty(&self) -> bool {
        self.removed.is_empty()
    }

    fn write_to_manifest(self, manifest_path: &Path) -> anyhow::Result<()> {
        if self.is_empty() {
            return Ok(());
        }

        let manifest = fs::read_to_string(manifest_path).with_context(|| {
            format!(
                "Could not read manifest at {} while restoring dev-dependencies",
                manifest_path.display()
            )
        })?;
        let mut document = manifest.parse::<DocumentMut>().with_context(|| {
            format!(
                "Could not parse manifest at {} while restoring dev-dependencies",
                manifest_path.display()
            )
        })?;

        let mut changed = false;
        let root = document.as_table_mut();
        for removed in self.removed {
            match removed.target {
                Some(target_name) => {
                    let targets = table_at_or_insert(root, "target")?;
                    let target = table_at_or_insert(targets, &target_name)?;
                    changed |= restore_removed_entry(target, removed.entry);
                }
                None => {
                    changed |= restore_removed_entry(root, removed.entry);
                }
            }
        }

        if changed {
            write_text_replace(manifest_path, &document.to_string()).with_context(|| {
                format!(
                    "Could not write manifest at {} while restoring dev-dependencies",
                    manifest_path.display()
                )
            })?;
        }

        Ok(())
    }

    fn write_restore_record(
        self,
        record_dir: &Path,
        manifest_path: &Path,
        original_manifest: &str,
    ) -> anyhow::Result<()> {
        let mut restore_document = DocumentMut::new();
        let root = restore_document.as_table_mut();
        for removed in self.removed {
            match removed.target {
                Some(target_name) => {
                    let targets = table_at_or_insert(root, "target")?;
                    let target = table_at_or_insert(targets, &target_name)?;
                    insert_removed_entry(target, removed.entry);
                }
                None => insert_removed_entry(root, removed.entry),
            }
        }

        fs::create_dir_all(record_dir).with_context(|| {
            format!(
                "Could not create dev-dependency restore directory at {}",
                record_dir.display()
            )
        })?;
        fs::write(record_dir.join(ORIGINAL_MANIFEST_FILE), original_manifest).with_context(
            || {
                format!(
                    "Could not write original manifest restore file at {}",
                    record_dir.display()
                )
            },
        )?;
        fs::write(
            record_dir.join(MANIFEST_PATH_FILE),
            format!("{}\n", manifest_path.display()),
        )
        .with_context(|| {
            format!(
                "Could not write dev-dependency restore manifest path at {}",
                record_dir.display()
            )
        })?;
        fs::write(
            record_dir.join(DEV_DEPENDENCY_RESTORE_FILE),
            restore_document.to_string(),
        )
        .with_context(|| {
            format!(
                "Could not write dev-dependency restore file at {}",
                record_dir.display()
            )
        })?;

        Ok(())
    }
}

fn table_at_or_insert<'a>(table: &'a mut Table, key: &str) -> anyhow::Result<&'a mut Table> {
    if !table.contains_key(key) {
        table.insert(key, Item::Table(Table::new()));
    }

    table
        .get_mut(key)
        .and_then(Item::as_table_mut)
        .ok_or_else(|| anyhow::anyhow!("manifest entry `{key}` is not a TOML table"))
}

fn insert_removed_entry(table: &mut Table, entry: RemovedTomlEntry) {
    table.insert_formatted(&entry.key, entry.item);
}

fn merge_missing_table_entries(existing: &mut Table, mut removed: Table) -> bool {
    let mut changed = false;
    let keys = Vec::from_iter(removed.iter().map(|(key, _)| key.to_owned()));
    for key in keys {
        if existing.contains_key(&key) {
            continue;
        }
        if let Some((removed_key, item)) = removed.remove_entry(&key) {
            existing.insert_formatted(&removed_key, item);
            changed = true;
        }
    }
    changed
}

fn restore_removed_entry(table: &mut Table, entry: RemovedTomlEntry) -> bool {
    let key = entry.key.get().to_owned();
    if !table.contains_key(&key) {
        insert_removed_entry(table, entry);
        return true;
    }

    let Some(existing) = table.get_mut(&key).and_then(Item::as_table_mut) else {
        return false;
    };
    match entry.item {
        Item::Table(removed) => merge_missing_table_entries(existing, removed),
        _ => false,
    }
}

fn write_text_replace(path: &Path, contents: &str) -> anyhow::Result<()> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("manifest");
    let temp_path =
        path.with_file_name(format!(".{file_name}.cargo-dragons-{}.tmp", process::id()));

    fs::write(&temp_path, contents).with_context(|| {
        format!(
            "Could not write temporary replacement file at {}",
            temp_path.display()
        )
    })?;
    fs::rename(&temp_path, path).with_context(|| {
        let _ = fs::remove_file(&temp_path);
        format!(
            "Could not replace {} with temporary file {}",
            path.display(),
            temp_path.display()
        )
    })?;
    Ok(())
}

/// Remove dev-dependency sections from a manifest.
pub fn strip_dev_dependencies_from_manifest(manifest_path: &Path) -> anyhow::Result<()> {
    let manifest = fs::read_to_string(manifest_path)
        .with_context(|| format!("Could not read manifest at {}", manifest_path.display()))?;
    let mut document = manifest
        .parse::<DocumentMut>()
        .with_context(|| format!("Could not parse manifest at {}", manifest_path.display()))?;
    let removed = collect_dev_dependency_sections(&mut document);

    if !removed.is_empty() {
        write_text_replace(manifest_path, &document.to_string())
            .with_context(|| format!("Could not write manifest at {}", manifest_path.display()))?;
    }

    Ok(())
}
