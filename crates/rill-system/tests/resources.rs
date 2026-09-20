//! Resource selection observes caller order, EOF and stale identities without fd assumptions.
use rill_system::resources::{DirectorySource, Item, Sources};
use std::time::Duration;

#[tokio::test(flavor = "current_thread")]
async fn selection_preserves_source_identity_and_closes_each_exhausted_input() {
    let directory = tempfile::tempdir().unwrap();
    for name in ["left", "right"] {
        std::fs::create_dir(directory.path().join(name)).unwrap();
        std::fs::write(directory.path().join(name).join("entry"), name).unwrap();
    }
    let cwd = rill_system::source::Directory::open(directory.path())
        .unwrap()
        .into_fd();
    let mut sources = Sources::default();
    let mut keys: Vec<_> = ["left", "right"]
        .into_iter()
        .map(|name| sources.register_directory(DirectorySource::open(&cwd, name.into()).unwrap()))
        .collect();
    let original = keys.clone();
    let result = tokio::time::timeout(Duration::from_secs(10), async {
        let mut names = vec!["left", "right"];
        let mut seen = Vec::new();
        while !keys.is_empty() {
            // Change selection order so an internal slot index cannot masquerade as a position.
            keys.reverse();
            names.reverse();
            let (index, item) = sources.next_ready(&keys, true).await.unwrap().unwrap();
            match item {
                Some(Item::Entry(entry)) => {
                    assert_eq!(entry.path, std::path::Path::new(names[index]).join("entry"));
                    assert_eq!(entry.kind, "file");
                    seen.push(names[index]);
                }
                None => {
                    keys.remove(index);
                    names.remove(index);
                }
                Some(_) => panic!("directory returned non-entry data"),
            }
        }
        seen.sort_unstable();
        assert_eq!(seen, ["left", "right"]);
        for key in original {
            assert!(sources.next(key).await.is_err());
            sources.close(key).await.unwrap();
        }
    })
    .await;
    sources.close_all().await.unwrap();
    result.expect("directory selection did not finish");
}

#[tokio::test(flavor = "current_thread")]
async fn invalid_selection_does_not_consume_live_resources() {
    let mut sources = Sources::default();
    let live = sources.external();
    let stale = sources.external();
    sources.close(stale).await.unwrap();
    for keys in [vec![], vec![live, live], vec![live, stale], vec![stale]] {
        assert!(sources.next_ready(&keys, false).await.is_err());
        assert!(sources.next_ready(&[live], false).await.unwrap().is_none());
    }
    sources.close_all().await.unwrap();
}
