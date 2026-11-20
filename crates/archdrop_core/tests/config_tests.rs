use archdrop_core::config::{expand_tilde, AppConfig, Config};
use std::path::PathBuf;

#[test]
fn test_default_config_values() {
    let cfg = Config::default();

    // Device defaults
    assert_eq!(cfg.device.device_type, "desktop");
    assert_eq!(cfg.device.device_model, "Arch Linux (Hyprland)");
    assert!(cfg.device.alias.is_none());

    // Network defaults
    assert_eq!(cfg.network.port, 53317);
    assert_eq!(cfg.network.bind_address, "0.0.0.0");
    assert!(!cfg.network.multicast_enabled);
    assert!(cfg.network.wireguard_peer_ip.is_none());

    // Security defaults
    assert!(cfg.security.require_encryption);
    assert!(cfg.security.auto_accept_paired);
    assert!(cfg.security.verify_checksums);
    assert!(cfg.security.paired_fingerprints.is_empty());
    assert!(cfg.security.pin.is_none());

    // Storage defaults
    assert_eq!(cfg.storage.download_dir, PathBuf::from("~/Downloads"));
    assert_eq!(cfg.storage.temp_dir, PathBuf::from("/tmp/archdrop"));

    // Clipboard defaults
    assert!(cfg.clipboard.enabled);
    assert_eq!(cfg.clipboard.inline_threshold_bytes, 4096);
    assert_eq!(cfg.clipboard.ttl_seconds, 120);
    assert!(cfg.clipboard.filter_secret_hints);
    assert!(cfg.clipboard.clear_on_lock);
}

#[test]
fn test_config_toml_roundtrip() {
    let mut cfg = Config::default();
    cfg.device.alias = Some("Custom Laptop".to_string());
    cfg.network.port = 54321;
    cfg.add_paired_fingerprint(
        "ABCDEF1234567890ABCDEF1234567890ABCDEF1234567890ABCDEF1234567890".to_string(),
    );

    let toml_str = toml::to_string(&cfg).expect("Serialization to TOML must succeed");
    let deserialized: Config =
        toml::from_str(&toml_str).expect("Deserialization from TOML must succeed");

    assert_eq!(deserialized.device.alias, Some("Custom Laptop".to_string()));
    assert_eq!(deserialized.network.port, 54321);
    assert_eq!(deserialized.security.paired_fingerprints.len(), 1);
    assert_eq!(deserialized.clipboard.inline_threshold_bytes, 4096);
}

#[test]
fn test_partial_toml_fallback_defaults() {
    let partial_toml = r#"
        [device]
        alias = "My Workstation"

        [network]
        port = 59999
    "#;

    let cfg: Config = toml::from_str(partial_toml).expect("Partial TOML must deserialize");
    assert_eq!(cfg.device.alias, Some("My Workstation".to_string()));
    assert_eq!(cfg.network.port, 59999);
    assert_eq!(cfg.network.bind_address, "0.0.0.0");
    assert_eq!(cfg.clipboard.inline_threshold_bytes, 4096);
    assert!(cfg.security.require_encryption);
}

#[test]
fn test_app_config_load_or_create() {
    let temp_dir = tempfile::tempdir().unwrap();
    let config_path = temp_dir.path().join("archdrop").join("config.toml");

    let app_config = AppConfig::load_or_create(&config_path).expect("load_or_create must succeed");
    assert!(config_path.exists(), "Config file must be created on disk");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta = std::fs::metadata(&config_path).unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
    }

    assert_eq!(app_config.config().network.port, 53317);
}

#[test]
fn test_config_field_aliases() {
    let toml_data = r#"
[storage]
download_directory = "/custom/downloads"

[clipboard]
threshold_bytes = 8192
auto_sync = false

[security]
token = "shared_secret_pin"
"#;
    let conf = Config::load_from_str(toml_data).expect("Aliases should parse cleanly");
    assert_eq!(
        conf.storage.download_dir,
        PathBuf::from("/custom/downloads")
    );
    assert_eq!(conf.clipboard.inline_threshold_bytes, 8192);
    assert!(!conf.clipboard.enabled);
    assert_eq!(conf.security.pin.as_deref(), Some("shared_secret_pin"));
}

#[test]
fn test_paired_fingerprints_management() {
    let mut conf = Config::default();
    let fp = "e7:89:12:ab:cd:ef:34:56".to_string();

    assert!(conf.add_paired_fingerprint(fp));
    // Duplicate insertion returns false
    assert!(!conf.add_paired_fingerprint("E7:89:12:AB:CD:EF:34:56".to_string()));

    // Case-insensitive verification
    assert!(conf.is_peer_paired("e7:89:12:ab:cd:ef:34:56"));
    assert!(conf.is_peer_paired("E7:89:12:AB:CD:EF:34:56"));
    assert!(!conf.is_peer_paired("00:00:00:00:00:00:00:00"));

    // Case-insensitive removal
    assert!(conf.remove_paired_fingerprint("e7:89:12:ab:cd:ef:34:56"));
    assert!(!conf.is_peer_paired("E7:89:12:AB:CD:EF:34:56"));
}

#[test]
fn test_expand_tilde_and_resolved_paths() {
    let mut conf = Config::default();
    conf.storage.download_dir = PathBuf::from("~/ArchDropReceived");
    conf.storage.temp_dir = PathBuf::from("~/ArchDropTemp");

    let resolved_dl = conf.resolved_download_dir();
    let resolved_tmp = conf.resolved_temp_dir();

    assert!(!resolved_dl.starts_with("~"));
    assert!(!resolved_tmp.starts_with("~"));

    if let Some(home) = dirs::home_dir() {
        assert!(resolved_dl.starts_with(&home));
        assert!(resolved_tmp.starts_with(&home));
    }

    let absolute = PathBuf::from("/var/archdrop");
    assert_eq!(expand_tilde(&absolute), absolute);
}

#[test]
fn test_effective_alias() {
    let mut conf = Config::default();
    conf.device.alias = Some("Custom Alias".to_string());
    assert_eq!(conf.effective_alias(), "Custom Alias");

    conf.device.alias = Some("   ".to_string());
    assert!(!conf.effective_alias().trim().is_empty());

    conf.device.alias = None;
    assert!(!conf.effective_alias().trim().is_empty());
}
