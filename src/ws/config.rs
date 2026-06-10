use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct ServeConfig {
    pub addr: SocketAddr,
    pub base_dir: PathBuf,
    pub max_concurrent_runs: usize,
    pub confirm_timeout: Duration,
}

impl Default for ServeConfig {
    fn default() -> Self {
        Self {
            addr: "0.0.0.0:8080".parse().unwrap(),
            base_dir: PathBuf::from(".maestro/runs"),
            max_concurrent_runs: 4,
            confirm_timeout: Duration::from_secs(30),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serve_config_default_values() {
        let cfg = ServeConfig::default();
        assert_eq!(cfg.addr, "0.0.0.0:8080".parse().unwrap());
        assert_eq!(cfg.base_dir, PathBuf::from(".maestro/runs"));
        assert_eq!(cfg.max_concurrent_runs, 4);
        assert_eq!(cfg.confirm_timeout, Duration::from_secs(30));
    }

    #[test]
    fn serve_config_clone_and_debug() {
        let cfg = ServeConfig::default();
        let cloned = cfg.clone();
        assert_eq!(cfg.addr, cloned.addr);
        let _ = format!("{:?}", cfg);
    }

    #[test]
    fn serve_config_custom_values() {
        let cfg = ServeConfig {
            addr: "127.0.0.1:3000".parse().unwrap(),
            base_dir: PathBuf::from("/tmp/runs"),
            max_concurrent_runs: 8,
            confirm_timeout: Duration::from_secs(60),
        };
        assert_eq!(cfg.max_concurrent_runs, 8);
    }
}
