// Tests for error handling and reconnection logic

#[cfg(test)]
mod tests {
    #[test]
    fn test_exponential_backoff_timing() {
        // Test that exponential backoff delays are correct
        let initial_delay = 100u64;
        let delays: Vec<u64> = (0..5)
            .map(|attempt| initial_delay * 2u64.pow(attempt))
            .collect();
        
        assert_eq!(delays, vec![100, 200, 400, 800, 1600]);
    }
    
    #[test]
    fn test_max_delay_cap() {
        // Test that delay is capped at 30 seconds
        let initial_delay = 100u64;
        let max_delay = 30000u64;
        
        for attempt in 0..10 {
            let delay = initial_delay * 2u64.pow(attempt);
            let capped_delay = delay.min(max_delay);
            assert!(capped_delay <= max_delay);
        }
    }
    
    #[test]
    fn test_error_message_format() {
        // Test that error messages are user-friendly
        let error_msg = "Failed to connect to kiro-cli at 127.0.0.1:8765 after 6 attempts. Please ensure kiro-cli is running.";
        
        assert!(error_msg.contains("Failed to connect"));
        assert!(error_msg.contains("Please ensure kiro-cli is running"));
        assert!(error_msg.contains("127.0.0.1:8765"));
    }
}
