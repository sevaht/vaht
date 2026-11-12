//! What the kernel reports about vaht's process: open descriptors and memory.

use std::collections::BTreeMap;
use std::fmt;

use procfs::process::{FDTarget, Process};

/// A snapshot of the process's open file descriptors and resident memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessUsage {
    /// Open descriptors, by kind.
    pub descriptors: DescriptorCensus,
    /// Resident memory in bytes, when the kernel reports it.
    pub resident_bytes: Option<u64>,
}

impl ProcessUsage {
    /// Bytes in the kilobytes `/proc/self/status` reports memory in.
    const BYTES_PER_KILOBYTE: u64 = 1024;

    /// Reads the current usage from `/proc/self`.
    ///
    /// Descriptors closed while the directory is read are skipped, not
    /// reported as errors.
    ///
    /// # Errors
    ///
    /// Returns an error if `/proc/self` cannot be read.
    pub fn read() -> Result<Self, ProcessUsageError> {
        let process = Process::myself().map_err(ProcessUsageError)?;
        let mut descriptors = DescriptorCensus::default();
        // Skipping errors drops descriptors closed between listing and reading
        // them: they are gone, which is the answer a census wants.
        for descriptor in process.fd().map_err(ProcessUsageError)?.flatten() {
            descriptors.add(DescriptorKind::from_target(&descriptor.target));
        }
        let resident_bytes = process
            .status()
            .map_err(ProcessUsageError)?
            .vmrss
            .map(|kilobytes| kilobytes * Self::BYTES_PER_KILOBYTE);
        Ok(Self {
            descriptors,
            resident_bytes,
        })
    }
}

impl fmt::Display for ProcessUsage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.descriptors)?;
        match self.resident_bytes {
            Some(bytes) => writeln!(formatter, "resident memory: {bytes} bytes"),
            None => writeln!(formatter, "resident memory: unknown"),
        }
    }
}

/// Counts of open file descriptors by kind.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DescriptorCensus {
    /// How many descriptors of each kind are open.
    count_by_kind: BTreeMap<DescriptorKind, usize>,
}

impl DescriptorCensus {
    /// Returns how many descriptors of `kind` are open.
    #[must_use]
    pub fn count(&self, kind: &DescriptorKind) -> usize {
        self.count_by_kind.get(kind).copied().unwrap_or_default()
    }

    /// Returns how many descriptors are open in all.
    #[must_use]
    pub fn total(&self) -> usize {
        self.count_by_kind.values().sum()
    }

    /// Counts one more descriptor of `kind`.
    fn add(&mut self, kind: DescriptorKind) {
        *self.count_by_kind.entry(kind).or_default() += 1;
    }
}

impl fmt::Display for DescriptorCensus {
    /// Formats the total, then one indented `kind: count` line per kind.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(formatter, "open descriptors: {}", self.total())?;
        for (kind, count) in &self.count_by_kind {
            writeln!(formatter, "  {kind}: {count}")?;
        }
        Ok(())
    }
}

/// What an open file descriptor refers to.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DescriptorKind {
    /// A regular file or directory.
    File,
    /// A device node under `/dev` (DRM, input devices).
    Device,
    /// A Unix or network socket (Wayland clients, IPC, D-Bus).
    Socket,
    /// A pipe.
    Pipe,
    /// An anonymous memory file (shared-memory buffers).
    Memory,
    /// An inode-less descriptor, by the name the kernel gives it: `eventfd`,
    /// `timerfd`, `pidfd`, `inotify`, `sync_file`, `dmabuf` and so on.
    Anonymous(String),
    /// Anything else.
    Other,
}

impl DescriptorKind {
    /// The directory device nodes live in.
    const DEVICE_DIRECTORY: &str = "/dev/";

    /// Classifies a descriptor by the target `/proc` reports for it.
    fn from_target(target: &FDTarget) -> Self {
        match target {
            FDTarget::Path(path) if path.starts_with(Self::DEVICE_DIRECTORY) => Self::Device,
            FDTarget::Path(_) => Self::File,
            FDTarget::Socket(_) | FDTarget::Net(_) => Self::Socket,
            FDTarget::Pipe(_) => Self::Pipe,
            FDTarget::MemFD(_) => Self::Memory,
            FDTarget::AnonInode(name) => Self::Anonymous(
                name.trim_start_matches('[')
                    .trim_end_matches(']')
                    .to_owned(),
            ),
            FDTarget::Other(..) | FDTarget::Unknown(..) => Self::Other,
        }
    }
}

impl fmt::Display for DescriptorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::File => formatter.write_str("file"),
            Self::Device => formatter.write_str("device"),
            Self::Socket => formatter.write_str("socket"),
            Self::Pipe => formatter.write_str("pipe"),
            Self::Memory => formatter.write_str("memory"),
            Self::Anonymous(name) => write!(formatter, "anonymous ({name})"),
            Self::Other => formatter.write_str("other"),
        }
    }
}

/// `/proc/self` could not be read.
#[derive(Debug, thiserror::Error)]
#[error("could not read this process's usage from /proc/self")]
pub struct ProcessUsageError(#[source] procfs::ProcError);

#[cfg(test)]
mod tests {
    use std::io::pipe;

    use super::*;

    #[test]
    fn a_pipe_appears_while_open_and_disappears_when_closed() {
        let before = ProcessUsage::read().expect("/proc/self is readable");
        let (reader, writer) = pipe().expect("a pipe can be created");
        let during = ProcessUsage::read().expect("/proc/self is readable");
        drop((reader, writer));
        let after = ProcessUsage::read().expect("/proc/self is readable");
        let pipes = |usage: &ProcessUsage| usage.descriptors.count(&DescriptorKind::Pipe);
        assert_eq!(pipes(&during), pipes(&before) + 2);
        assert_eq!(pipes(&after), pipes(&before));
    }

    #[test]
    fn resident_memory_is_reported() {
        let usage = ProcessUsage::read().expect("/proc/self is readable");
        assert!(usage.resident_bytes.is_some_and(|bytes| bytes > 0));
    }

    #[test]
    fn device_paths_are_devices_and_anonymous_names_lose_their_brackets() {
        assert_eq!(
            DescriptorKind::from_target(&FDTarget::Path("/dev/dri/card1".into())),
            DescriptorKind::Device
        );
        assert_eq!(
            DescriptorKind::from_target(&FDTarget::Path("/home/user/vaht.log".into())),
            DescriptorKind::File
        );
        assert_eq!(
            DescriptorKind::from_target(&FDTarget::AnonInode("[eventfd]".into())),
            DescriptorKind::Anonymous("eventfd".into())
        );
    }
}
