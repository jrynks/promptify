use std::sync::OnceLock;

use serde::Serialize;

const BYTES_PER_GB: u64 = 1_000_000_000;

#[derive(Serialize)]
pub struct ModelCompatibility {
    pub supported: Option<bool>,
    pub total_ram_bytes: Option<u64>,
    pub reason: Option<String>,
}

fn total_ram() -> Result<u64, String> {
    static MEMORY: OnceLock<Result<u64, String>> = OnceLock::new();
    MEMORY
        .get_or_init(|| {
            let mut system = sysinfo::System::new();
            system.refresh_memory();
            match system.total_memory() {
                0 => {
                    let error =
                        "Could not detect total system RAM; model compatibility is unknown."
                            .to_owned();
                    log::warn!("{error}");
                    Err(error)
                }
                bytes => Ok(bytes),
            }
        })
        .clone()
}

pub fn model_compatibility(min_ram_gb: u32) -> ModelCompatibility {
    compare_ram(min_ram_gb, total_ram())
}

fn compare_ram(min_ram_gb: u32, detected: Result<u64, String>) -> ModelCompatibility {
    match detected {
        Ok(bytes) => {
            let supported = bytes >= u64::from(min_ram_gb) * BYTES_PER_GB;
            ModelCompatibility {
                supported: Some(supported),
                total_ram_bytes: Some(bytes),
                reason: (!supported).then(|| {
                    format!(
                        "Below minimum RAM: requires {min_ram_gb} GB; this computer has {:.1} GB.",
                        bytes as f64 / BYTES_PER_GB as f64,
                    )
                }),
            }
        }
        Err(error) => ModelCompatibility {
            supported: None,
            total_ram_bytes: None,
            reason: Some(error),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ram_requirement_uses_exact_bytes_not_rounded_display_values() {
        assert_eq!(compare_ram(8, Ok(8 * BYTES_PER_GB)).supported, Some(true));
        let below = compare_ram(8, Ok(8 * BYTES_PER_GB - 1));
        assert_eq!(below.supported, Some(false));
        assert!(below.reason.unwrap().contains("requires 8 GB"));
        assert_eq!(compare_ram(16, Ok(8 * BYTES_PER_GB)).supported, Some(false));
        assert_eq!(compare_ram(4, Ok(8 * BYTES_PER_GB)).supported, Some(true));
    }

    #[test]
    fn missing_hardware_is_unknown_not_unsupported() {
        let result = compare_ram(8, Err("Detection failed".into()));
        assert_eq!(result.supported, None);
        assert_eq!(result.total_ram_bytes, None);
        assert_eq!(result.reason.as_deref(), Some("Detection failed"));
    }

    #[test]
    fn live_system_memory_is_detected() {
        assert!(total_ram().unwrap() > 0);
    }
}
