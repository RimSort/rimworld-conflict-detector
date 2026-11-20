use std::io;
use std::path::Path;

pub fn read_xml_file(path: &Path) -> io::Result<String> {
    std::fs::read_to_string(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::ErrorKind;

    #[test]
    fn reports_missing_and_invalid_utf8_inputs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.xml");
        assert_eq!(
            read_xml_file(&path).unwrap_err().kind(),
            ErrorKind::NotFound
        );
        std::fs::write(&path, [0xff]).unwrap();
        assert_eq!(
            read_xml_file(&path).unwrap_err().kind(),
            ErrorKind::InvalidData
        );
    }

    #[test]
    fn reads_xml_larger_than_former_limit() {
        use std::io::{self, Read, Write};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.xml");
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(b"<root><!--").unwrap();
        io::copy(&mut io::repeat(b'x').take(50 * 1024 * 1024 + 1), &mut file).unwrap();
        file.write_all(b"--></root>").unwrap();
        drop(file);
        let xml = read_xml_file(&path).unwrap();
        assert!(xml.len() > 50 * 1024 * 1024);
        assert!(xml.ends_with("--></root>"));
    }

    #[test]
    fn reads_multibyte_utf8_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("multibyte.xml");
        let expected = "<根>世界</根>";
        std::fs::write(&path, expected).unwrap();

        assert_eq!(read_xml_file(&path).unwrap(), expected);
    }
}
