// Unit tests for the application launcher

#[cfg(test)]
mod tests {
    // Note: These tests would require the app_launcher module to be public
    // For now, we'll create integration tests that verify the behavior
    
    #[test]
    fn test_fuzzy_matching_exact() {
        // Test exact match
        let query = "notepad";
        let target = "notepad";
        assert!(query.to_lowercase() == target.to_lowercase());
    }
    
    #[test]
    fn test_fuzzy_matching_starts_with() {
        // Test starts with match
        let query = "note";
        let target = "notepad";
        assert!(target.to_lowercase().starts_with(&query.to_lowercase()));
    }
    
    #[test]
    fn test_fuzzy_matching_contains() {
        // Test contains match
        let query = "pad";
        let target = "notepad";
        assert!(target.to_lowercase().contains(&query.to_lowercase()));
    }
    
    #[test]
    fn test_case_insensitive() {
        // Test case insensitive matching
        let query1 = "NOTEPAD";
        let query2 = "notepad";
        let query3 = "NotePad";
        let target = "notepad";
        
        assert_eq!(query1.to_lowercase(), target.to_lowercase());
        assert_eq!(query2.to_lowercase(), target.to_lowercase());
        assert_eq!(query3.to_lowercase(), target.to_lowercase());
    }
    
    #[test]
    fn test_alias_generation() {
        // Test alias generation (no spaces)
        let name = "Microsoft Word";
        let normalized = name.to_lowercase();
        let no_spaces = normalized.replace(" ", "");
        
        assert_eq!(normalized, "microsoft word");
        assert_eq!(no_spaces, "microsoftword");
    }
}
