//! Reedline owns persistence and navigation; this adapter rejects lossy history entries.
use reedline::{
    FileBackedHistory, History, HistoryItem, HistoryItemId, HistorySessionId, SearchQuery,
};
use std::{
    fs, io,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::Path,
};

pub const ENTRY_LIMIT: usize = 1024 * 1024;
const ENTRIES: usize = 10_000;

pub fn storable(source: &str) -> bool {
    source.len() <= ENTRY_LIMIT && !source.contains("<\\n>")
}

pub fn open(path: &Path) -> io::Result<SafeHistory> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("history has no state directory"))?;
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    if fs::symlink_metadata(path).is_ok_and(|metadata| !metadata.is_file()) {
        return Err(io::Error::other("history must be a regular file"));
    }
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)?;
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    FileBackedHistory::with_file(ENTRIES, path.into())
        .map(SafeHistory)
        .map_err(io::Error::other)
}

pub struct SafeHistory(pub FileBackedHistory);
impl Default for SafeHistory {
    fn default() -> Self {
        Self(FileBackedHistory::new(ENTRIES).expect("finite history capacity"))
    }
}
impl History for SafeHistory {
    fn save(&mut self, item: HistoryItem) -> reedline::Result<HistoryItem> {
        if storable(&item.command_line) {
            self.0.save(item)
        } else {
            Ok(item)
        }
    }
    fn load(&self, id: HistoryItemId) -> reedline::Result<HistoryItem> {
        self.0.load(id)
    }
    fn count(&self, query: SearchQuery) -> reedline::Result<i64> {
        self.0.count(query)
    }
    fn search(&self, query: SearchQuery) -> reedline::Result<Vec<HistoryItem>> {
        self.0.search(query)
    }
    fn update(
        &mut self,
        id: HistoryItemId,
        update: &dyn Fn(HistoryItem) -> HistoryItem,
    ) -> reedline::Result<()> {
        self.0.update(id, update)
    }
    fn clear(&mut self) -> reedline::Result<()> {
        self.0.clear()
    }
    fn delete(&mut self, id: HistoryItemId) -> reedline::Result<()> {
        self.0.delete(id)
    }
    fn sync(&mut self) -> io::Result<()> {
        self.0.sync()
    }
    fn session(&self) -> Option<HistorySessionId> {
        self.0.session()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_persistence_preserves_multiline_source_and_private_permissions() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("history");
        let source = "do {\n  \"literal\\n\"\n}";
        let mut history = open(&path).unwrap();
        history
            .save(HistoryItem::from_command_line(source))
            .unwrap();
        history
            .save(HistoryItem::from_command_line("\"<\\n>\""))
            .unwrap();
        history.sync().unwrap();
        drop(history);
        let history = open(&path).unwrap();
        let entries = history
            .search(SearchQuery::everything(
                reedline::SearchDirection::Forward,
                None,
            ))
            .unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].command_line, source);
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(directory.path()).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }

    #[test]
    fn native_history_never_receives_entries_its_format_would_change() {
        let mut history = SafeHistory::default();
        for source in ["print \"<\\n>\"".into(), "x".repeat(ENTRY_LIMIT + 1)] {
            let item = history
                .save(HistoryItem::from_command_line(source))
                .unwrap();
            assert!(item.id.is_none());
        }
        let source = "do {\n  42\n}";
        let saved = history
            .save(HistoryItem::from_command_line(source))
            .unwrap();
        assert_eq!(
            history.load(saved.id.unwrap()).unwrap().command_line,
            source
        );
        assert_eq!(history.count_all().unwrap(), 1);
    }
}
