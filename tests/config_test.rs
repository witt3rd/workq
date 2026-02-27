use animus_rs::config::Config;

#[test]
fn config_from_env_loads_required_fields() {
    // Set required env vars for test
    unsafe {
        std::env::set_var("DATABASE_URL", "postgres://test:test@localhost/test");
        std::env::set_var("ANTHROPIC_API_KEY", "sk-test-key");
    }

    let config = Config::from_env().unwrap();
    assert!(!config.log_level.is_empty());

    // Clean up
    unsafe {
        std::env::remove_var("DATABASE_URL");
        std::env::remove_var("ANTHROPIC_API_KEY");
    }
}

#[test]
fn config_from_env_fails_without_required() {
    unsafe {
        std::env::remove_var("DATABASE_URL");
        std::env::remove_var("ANTHROPIC_API_KEY");
    }

    let result = Config::from_env();
    assert!(result.is_err());
}
