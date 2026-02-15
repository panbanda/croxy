use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

use crate::config::MetricsLogConfig;

pub struct MetricsLogger {
    path: PathBuf,
    max_size: u64,
    max_files: u32,
    writer: BufWriter<File>,
}

impl MetricsLogger {
    pub fn new(config: &MetricsLogConfig) -> io::Result<Self> {
        let path = PathBuf::from(&config.path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Self {
            path,
            max_size: config.max_size_mb * 1024 * 1024,
            max_files: config.max_files,
            writer: BufWriter::new(file),
        })
    }

    pub fn write_line(&mut self, line: &str) -> io::Result<()> {
        writeln!(self.writer, "{line}")?;
        self.writer.flush()?;
        self.maybe_rotate()
    }

    fn maybe_rotate(&mut self) -> io::Result<()> {
        let size = fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0);
        if size < self.max_size {
            return Ok(());
        }
        self.rotate()
    }

    fn rotate(&mut self) -> io::Result<()> {
        // Delete the oldest file if it would exceed max_files
        let oldest = rotated_path(&self.path, self.max_files);
        if oldest.exists() {
            fs::remove_file(&oldest)?;
        }

        // Shift existing rotated files: .N-1 -> .N, ..., .1 -> .2
        for i in (1..self.max_files).rev() {
            let from = rotated_path(&self.path, i);
            let to = rotated_path(&self.path, i + 1);
            if from.exists() {
                fs::rename(&from, &to)?;
            }
        }

        // Rename current to .1
        let first_rotated = rotated_path(&self.path, 1);
        fs::rename(&self.path, &first_rotated)?;

        // Open a fresh file
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        self.writer = BufWriter::new(file);

        Ok(())
    }
}

pub(crate) fn rotated_path(base: &Path, index: u32) -> PathBuf {
    let name = base.file_name().unwrap_or_default().to_string_lossy();
    base.with_file_name(format!("{name}.{index}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config(dir: &Path, max_size_mb: u64, max_files: u32) -> MetricsLogConfig {
        MetricsLogConfig {
            enabled: true,
            path: dir.join("metrics.jsonl").to_string_lossy().to_string(),
            max_size_mb,
            max_files,
        }
    }

    #[test]
    fn writes_and_reads_back_lines() {
        let dir = tempfile::tempdir().unwrap();
        let config = test_config(dir.path(), 50, 5);
        let mut logger = MetricsLogger::new(&config).unwrap();

        logger
            .write_line(r#"{"model":"opus","status":200}"#)
            .unwrap();
        logger
            .write_line(r#"{"model":"sonnet","status":429}"#)
            .unwrap();

        let content = fs::read_to_string(dir.path().join("metrics.jsonl")).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("opus"));
        assert!(lines[1].contains("sonnet"));
    }

    #[test]
    fn rotates_when_size_exceeded() {
        let dir = tempfile::tempdir().unwrap();
        // max_size will be 0 * 1MB = 0, so any write triggers rotation
        let config = test_config(dir.path(), 0, 3);
        let mut logger = MetricsLogger::new(&config).unwrap();

        logger.write_line("line1").unwrap();
        assert!(dir.path().join("metrics.jsonl.1").exists());

        logger.write_line("line2").unwrap();
        assert!(dir.path().join("metrics.jsonl.2").exists());
        assert!(dir.path().join("metrics.jsonl.1").exists());

        // Verify rotated content
        let rotated = fs::read_to_string(dir.path().join("metrics.jsonl.2")).unwrap();
        assert!(rotated.contains("line1"));
    }

    #[test]
    fn respects_max_files_limit() {
        let dir = tempfile::tempdir().unwrap();
        let config = test_config(dir.path(), 0, 2);
        let mut logger = MetricsLogger::new(&config).unwrap();

        logger.write_line("line1").unwrap();
        logger.write_line("line2").unwrap();
        logger.write_line("line3").unwrap();

        // .1 and .2 should exist, but not .3 (max_files=2 rotated + current)
        assert!(dir.path().join("metrics.jsonl").exists());
        assert!(dir.path().join("metrics.jsonl.1").exists());
        assert!(dir.path().join("metrics.jsonl.2").exists());
        assert!(!dir.path().join("metrics.jsonl.3").exists());

        // The oldest (line1) should have been deleted
        let r2 = fs::read_to_string(dir.path().join("metrics.jsonl.2")).unwrap();
        assert!(
            r2.contains("line2"),
            "oldest rotated should be line2, got: {r2}"
        );
    }

    #[test]
    fn creates_parent_directories() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a/b/c/metrics.jsonl");
        let config = MetricsLogConfig {
            enabled: true,
            path: nested.to_string_lossy().to_string(),
            max_size_mb: 50,
            max_files: 5,
        };
        let mut logger = MetricsLogger::new(&config).unwrap();
        logger.write_line("test").unwrap();
        assert!(nested.exists());
    }

    #[test]
    fn appends_to_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("metrics.jsonl");
        fs::write(&path, "existing\n").unwrap();

        let config = test_config(dir.path(), 50, 5);
        let mut logger = MetricsLogger::new(&config).unwrap();
        logger.write_line("new").unwrap();

        let content = fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "existing");
        assert_eq!(lines[1], "new");
    }
}
