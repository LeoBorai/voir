use std::fs::File;
use std::io::BufReader;
use std::path::Path;

/// Opens a File from the Filesystem and retrieve it's bytes
pub fn open_file<P: AsRef<Path>>(path: P) -> BufReader<File> {
    let file = File::open(path).expect("Failed to find file");
    let buffer = BufReader::new(file);

    return buffer;
}
