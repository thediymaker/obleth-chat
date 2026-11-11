//! What one conversation may cost while it is read. An export is another app's data
//! in a shape Chat does not control, and a small file can ask for a great deal: an
//! object of empty messages takes about a hundred times its own length once parsed,
//! and a zip inflates a thousandfold. So every byte read for a conversation and
//! every byte kept from it is charged to one allowance, and reading stops when a
//! conversation goes over.
//!
//! The allowance belongs to the thread doing the reading, which is how the field
//! readers in `formats` reach it without each carrying it.
use std::cell::Cell;
use std::io::Read;

/// One conversation's allowance. Chat keeps 4 MiB of a conversation; the rest is
/// room for branches and steps that are folded away as it is shaped.
const RECORD: usize = 64 * 1024 * 1024;

thread_local! {
    static LEFT: Cell<usize> = const { Cell::new(usize::MAX) };
    static OVER: Cell<bool> = const { Cell::new(false) };
}

/// Holds this thread to the allowance until dropped.
pub(super) struct Limit(());

pub(super) fn limit() -> Limit {
    OVER.set(false);
    LEFT.set(RECORD);
    Limit(())
}
impl Limit {
    /// Whether reading stopped because a conversation went over.
    pub(super) fn over(&self) -> bool {
        OVER.get()
    }
}
impl Drop for Limit {
    fn drop(&mut self) {
        LEFT.set(usize::MAX);
    }
}

/// Starts the next conversation with a full allowance.
pub(super) fn next_record() {
    if LEFT.get() != usize::MAX {
        LEFT.set(RECORD);
    }
}

fn take(bytes: usize) -> bool {
    match LEFT.get().checked_sub(bytes) {
        Some(left) => {
            LEFT.set(left);
            true
        }
        None => {
            OVER.set(true);
            false
        }
    }
}

/// Charges memory about to be kept.
pub(super) fn charge<E: serde::de::Error>(bytes: usize) -> Result<(), E> {
    if take(bytes) {
        Ok(())
    } else {
        Err(E::custom("a conversation larger than Chat reads"))
    }
}

/// Charges one more `T` in a list, with the spare room a growing list keeps.
pub(super) fn slot<T, E: serde::de::Error>() -> Result<(), E> {
    charge(2 * std::mem::size_of::<T>())
}

/// Charges the bytes of the file as they are read.
pub(super) struct Charged<R>(pub R);
impl<R: Read> Read for Charged<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.0.read(buf)?;
        if take(n) {
            Ok(n)
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "a conversation larger than Chat reads",
            ))
        }
    }
}
