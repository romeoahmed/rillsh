//! Globbing preserves native bytes and uses POSIX pattern rules without locale sorting.
use std::{
    ffi::{CString, OsString},
    os::unix::ffi::{OsStrExt, OsStringExt},
    path::PathBuf,
};
#[test]
fn posix_patterns_exclude_hidden_names_and_sort_by_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let base = directory.path().join("data");
    std::fs::create_dir(&base).unwrap();
    for name in [b"b".as_slice(), b"a", b".hidden", "\u{1f30a}".as_bytes()] {
        std::fs::write(base.join(OsString::from_vec(name.to_vec())), []).unwrap();
    }
    let expand = |pattern: &[u8]| {
        let mut absolute = base.as_os_str().as_bytes().to_vec();
        absolute.push(b'/');
        absolute.extend_from_slice(pattern);
        rill_system::glob::expand(&CString::new(absolute).unwrap()).map(|paths| {
            paths
                .into_iter()
                .map(|path| path.strip_prefix(&base).unwrap().to_path_buf())
                .collect::<Vec<_>>()
        })
    };
    let paths = expand(b"*").unwrap();
    assert_eq!(
        paths
            .iter()
            .map(|path| path.as_os_str().as_bytes())
            .collect::<Vec<_>>(),
        [b"a".as_slice(), b"b", "\u{1f30a}".as_bytes()]
    );
    assert_eq!(expand(b".[a-z]*").unwrap(), [PathBuf::from(".hidden")]);
    assert!(rill_system::glob::expand(c"").unwrap().is_empty());
    assert!(expand(b"missing*").unwrap().is_empty());
}
