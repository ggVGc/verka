use super::*;

#[test]
fn log_lives_beside_socket() {
    assert_eq!(
        path_for_socket(Path::new("/run/user/1000/styra/styra.sock")),
        PathBuf::from("/run/user/1000/styra/styra-tui.log")
    );
}

#[test]
fn formatter_fragments_become_one_record() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("styra-tui.log");
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .unwrap();
    let factory = LineWriterFactory::new(file);
    let mut writer = factory.make_writer();

    writer.write_all(b"first ").unwrap();
    writer.write_all(b"record\n").unwrap();
    drop(writer);

    assert_eq!(fs::read_to_string(path).unwrap(), "first record\n");
}
