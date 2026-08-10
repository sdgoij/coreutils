// This file is part of the uutils coreutils package.
//
// For the full copyright and license information, please view the LICENSE
// file that was distributed with this source code.

// spell-checker:ignore lseek

use std::fs::File;
use std::io::{self, Seek, SeekFrom};
use std::os::fd::AsFd;
use uucore::fs::FileInformation;

/// An unsafe overwrite occurs when the same nonempty file is used as both stdin and stdout,
/// and the file offset of stdin is positioned earlier than that of stdout.
/// In this scenario, bytes read from stdin are written to a later part of the file
/// via stdout, which can then be read again by stdin and written again by stdout,
/// causing an infinite loop and potential file corruption.
///
/// Minix: std does not expose fcntl F_GETFL, so the O_APPEND case is not
/// distinguished; the conservative non-append check still rejects the
/// corruption case.
pub fn is_safe_overwrite<I: AsFd, O: AsFd>(input: &I, output: &O) -> bool {
    let Ok(input_info) = FileInformation::from_file(input) else {
        return true;
    };
    let Ok(output_info) = FileInformation::from_file(output) else {
        return true;
    };
    if input_info != output_info {
        return true;
    }
    let file_size = output_info.file_size();
    if file_size == 0 {
        return true;
    }
    let input_pos = seek_pos(input);
    let output_pos = seek_pos(output);
    let (Ok(input_pos), Ok(output_pos)) = (input_pos, output_pos) else {
        return true;
    };
    input_pos >= output_pos
}

/// The current offset of the open file description shared by `f` and its
/// cloned descriptor.
fn seek_pos<F: AsFd>(f: &F) -> io::Result<u64> {
    let mut file = File::from(f.as_fd().try_clone_to_owned()?);
    file.seek(SeekFrom::Current(0))
}
