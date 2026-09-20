//! Regular-file capabilities, UTF-8 decoding and caller-selected source limits.
use rill_system::source::Directory;
use std::{fs, io::ErrorKind, path::Path};

#[test]
fn source_reads_are_bounded_and_require_utf8() {
    let temporary = tempfile::tempdir().unwrap();
    fs::write(temporary.path().join("source.rill"), "\u{1f30a}").unwrap();
    let directory = Directory::open(temporary.path()).unwrap();
    for limit in 0..4 {
        assert_eq!(
            directory
                .source(Path::new("source.rill"))
                .unwrap()
                .read(limit)
                .unwrap_err()
                .kind(),
            ErrorKind::FileTooLarge
        );
    }
    assert_eq!(
        directory
            .source(Path::new("source.rill"))
            .unwrap()
            .read(4)
            .unwrap(),
        "\u{1f30a}"
    );
    fs::write(temporary.path().join("invalid.rill"), [0xff]).unwrap();
    assert_eq!(
        directory
            .source(Path::new("invalid.rill"))
            .unwrap()
            .read(4)
            .unwrap_err()
            .kind(),
        ErrorKind::InvalidData
    );
    fs::write(temporary.path().join("empty.rill"), "").unwrap();
    assert_eq!(
        directory
            .source(Path::new("empty.rill"))
            .unwrap()
            .read(0)
            .unwrap(),
        ""
    );
}

#[test]
fn directories_and_fifos_are_not_source_files() {
    let temporary = tempfile::tempdir().unwrap();
    fs::create_dir(temporary.path().join("directory")).unwrap();
    assert!(
        std::process::Command::new("mkfifo")
            .arg(temporary.path().join("fifo"))
            .status()
            .unwrap()
            .success()
    );
    let directory = Directory::open(temporary.path()).unwrap();
    for name in ["directory", "fifo"] {
        let error = directory
            .source(Path::new(name))
            .expect_err("non-regular file was accepted");
        assert_eq!(error.kind(), ErrorKind::InvalidInput);
    }
}
