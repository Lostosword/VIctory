//! Whole-file reads that ask the host for nothing but the bytes.
//!
//! The standard library's `fs::read`, `Path::is_file` and `Path::exists`
//! ask for a file's information by handle, and on Windows that includes its
//! volume's serial number. Under Wine that question is a device request to
//! the mount manager in another process, answered through a signal to the
//! asking thread: about a millisecond each, several thousand of them while
//! a title's shaders come from the cache. Under Rosetta 2 the signal is
//! sometimes delivered inside the thread's next server call, which then
//! never returns and keeps the lock on the address space: every thread that
//! maps, allocates or queries memory waits for it for good (seen twice with
//! a shader worker, in its second such read in a row).
//!
//! Here the size comes from seeking to the end, which asks nobody.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// The file's bytes.
pub fn read(path: &Path) -> std::io::Result<Vec<u8>> {
    let mut file = std::fs::File::open(path)?;
    let len = file.seek(SeekFrom::End(0))?;
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = vec![0; len as usize];
    file.read_exact(&mut bytes)?;
    // A file that grew meanwhile: the rest, as `fs::read` would give it.
    let mut rest = [0u8; 4096];
    loop {
        match file.read(&mut rest)? {
            0 => return Ok(bytes),
            n => bytes.extend_from_slice(&rest[..n]),
        }
    }
}

/// The file's text.
pub fn read_to_string(path: &Path) -> std::io::Result<String> {
    String::from_utf8(read(path)?)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

/// Whether `path` is a file that can be opened for reading (a directory
/// cannot be opened as one).
pub fn is_file(path: &Path) -> bool {
    std::fs::File::open(path).is_ok_and(|mut file| {
        // Unix opens directories too; reading one fails.
        cfg!(windows) || file.read(&mut [0u8; 1]).is_ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_the_standard_library_reads() {
        let dir = std::env::temp_dir().join(format!("victory-files-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for len in [0usize, 1, 4095, 4096, 4097, 100_000] {
            let path = dir.join(format!("f{len}"));
            let data: Vec<u8> = (0..len).map(|i| (i * 7 + len) as u8).collect();
            std::fs::write(&path, &data).unwrap();
            assert_eq!(read(&path).unwrap(), data);
            assert!(is_file(&path));
        }
        let text = dir.join("text");
        std::fs::write(&text, "12:Enter\n").unwrap();
        assert_eq!(read_to_string(&text).unwrap(), "12:Enter\n");
        std::fs::write(&text, [0xff, 0xfe]).unwrap();
        assert!(read_to_string(&text).is_err());
        assert!(!is_file(&dir));
        assert!(!is_file(&dir.join("absent")));
        assert!(read(&dir.join("absent")).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
