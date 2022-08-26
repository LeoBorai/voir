use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;
use std::str::from_utf8_unchecked;

pub struct Voir {
    pub(crate) buffer: Option<BufReader<File>>,
    pub(crate) bytes_per_line: usize,
}

impl Voir {
    pub fn new(buffer: Option<BufReader<File>>, bytes_per_line: usize) -> Self {
        Self {
            buffer,
            bytes_per_line,
        }
    }

    pub fn open<P: AsRef<Path>>(&mut self, filename: P) {
        let file = File::open(filename).expect("Failed to open the file");
        let buffer = BufReader::new(file);

        self.buffer = Some(buffer);
    }

    pub fn hex(self) {
        let mut position = 0;
        let mut bytes = Vec::new();

        self.buffer.unwrap().read_to_end(&mut bytes).unwrap();

        for line in bytes.chunks(self.bytes_per_line) {
            let mut bytes_in_line = Vec::with_capacity(self.bytes_per_line);

            print!("0x{:06x}\t", position);

            for byte in line {
                print!("{:02x} ", byte);

                if *byte != 13 && *byte != 10 && *byte != 9 {
                    bytes_in_line.push(*byte);
                }
            }

            print!("  {}", unsafe {
                from_utf8_unchecked(bytes_in_line.as_slice())
            });
            print!("\n");
            position += self.bytes_per_line;

            if position >= 5000 {
                break;
            }
        }
    }
}

impl Default for Voir {
    fn default() -> Self {
        Self {
            buffer: None,
            bytes_per_line: 16,
        }
    }
}
