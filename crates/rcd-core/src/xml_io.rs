use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

pub const MAX_XML_FILE_BYTES: u64 = 50 * 1024 * 1024;

pub fn read_xml_file(path: &Path) -> io::Result<String> {
    let file = File::open(path)?;
    if file.metadata()?.len() > MAX_XML_FILE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "XML file exceeds the 50 MiB size limit",
        ));
    }

    let mut bytes = Vec::new();
    file.take(MAX_XML_FILE_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_XML_FILE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "XML file exceeds the 50 MiB size limit",
        ));
    }

    String::from_utf8(bytes).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::ErrorKind;

    #[test]
    fn reads_file_exactly_at_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("at-limit.xml");
        std::fs::write(&path, vec![b'x'; MAX_XML_FILE_BYTES as usize]).unwrap();

        let content = read_xml_file(&path).unwrap();

        assert_eq!(content.len() as u64, MAX_XML_FILE_BYTES);
    }

    #[test]
    fn rejects_file_over_limit_with_invalid_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("over-limit.xml");
        std::fs::write(&path, vec![b'x'; MAX_XML_FILE_BYTES as usize + 1]).unwrap();

        let error = read_xml_file(&path).unwrap_err();

        assert_eq!(error.kind(), ErrorKind::InvalidData);
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
