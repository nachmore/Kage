use anyhow::{Context, Result};
use chrono::Local;
use log::{Level, LevelFilter, Metadata, Record};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

const MAX_LOG_SIZE: u64 = 10 * 1024 * 1024; // 10MB
const MAX_LOG_FILES: usize = 5;

pub struct FileLogger {
    log_file: Mutex<File>,
    log_path: PathBuf,
}

impl FileLogger {
    pub fn new() -> Result<Self> {
        let log_path = Self::get_log_path()?;
        
        // Create log directory if it doesn't exist
        if let Some(parent) = log_path.parent() {
            fs::create_dir_all(parent)
                .context("Failed to create log directory")?;
        }
        
        // Rotate logs if needed
        Self::rotate_logs_if_needed(&log_path)?;
        
        // Open or create log file
        let log_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .context("Failed to open log file")?;
        
        Ok(Self {
            log_file: Mutex::new(log_file),
            log_path,
        })
    }
    
    fn get_log_path() -> Result<PathBuf> {
        let log_dir = dirs::data_local_dir()
            .context("Failed to get local data directory")?
            .join("kiro-assistant")
            .join("logs");
        
        Ok(log_dir.join("kiro-assistant.log"))
    }
    
    fn rotate_logs_if_needed(log_path: &PathBuf) -> Result<()> {
        // Check if log file exists and its size
        if !log_path.exists() {
            return Ok(());
        }
        
        let metadata = fs::metadata(log_path)
            .context("Failed to get log file metadata")?;
        
        if metadata.len() < MAX_LOG_SIZE {
            return Ok(());
        }
        
        // Rotate existing logs
        for i in (1..MAX_LOG_FILES).rev() {
            let old_path = if i == 1 {
                log_path.clone()
            } else {
                log_path.with_extension(format!("log.{}", i - 1))
            };
            
            let new_path = log_path.with_extension(format!("log.{}", i));
            
            if old_path.exists() {
                if i == MAX_LOG_FILES - 1 {
                    // Delete oldest log
                    fs::remove_file(&old_path)
                        .context("Failed to remove old log file")?;
                } else {
                    // Rename to next number
                    fs::rename(&old_path, &new_path)
                        .context("Failed to rotate log file")?;
                }
            }
        }
        
        // Create new empty log file
        File::create(log_path)
            .context("Failed to create new log file")?;
        
        Ok(())
    }
    
    fn log_to_file(&self, record: &Record) {
        let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
        let level = record.level();
        let target = record.target();
        let message = record.args();
        
        let log_line = format!(
            "[{}] {} [{}] {}\n",
            timestamp, level, target, message
        );
        
        if let Ok(mut file) = self.log_file.lock() {
            let _ = file.write_all(log_line.as_bytes());
            let _ = file.flush();
            
            // Check if rotation is needed after writing
            if let Ok(metadata) = file.metadata() {
                if metadata.len() >= MAX_LOG_SIZE {
                    drop(file); // Release lock before rotating
                    let _ = Self::rotate_logs_if_needed(&self.log_path);
                    
                    // Reopen file
                    if let Ok(new_file) = OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&self.log_path)
                    {
                        if let Ok(mut file_lock) = self.log_file.lock() {
                            *file_lock = new_file;
                        }
                    }
                }
            }
        }
    }
}

impl log::Log for FileLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= Level::Info
    }
    
    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            // Log to file
            self.log_to_file(record);
            
            // Also log to console for errors and warnings
            if record.level() <= Level::Warn {
                eprintln!("[{}] {}", record.level(), record.args());
            }
        }
    }
    
    fn flush(&self) {
        if let Ok(mut file) = self.log_file.lock() {
            let _ = file.flush();
        }
    }
}

pub fn init_logger() -> Result<()> {
    let logger = FileLogger::new()
        .context("Failed to initialize file logger")?;
    
    log::set_boxed_logger(Box::new(logger))
        .context("Failed to set logger")?;
    
    log::set_max_level(LevelFilter::Info);
    
    log::info!("Kiro Assistant started");
    log::info!("Log file: {:?}", FileLogger::get_log_path()?);
    
    Ok(())
}
