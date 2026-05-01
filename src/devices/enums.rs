use crate::devices::airpods::AirPodsInformation;
use serde::{Deserialize, Serialize};
use std::fmt::Display;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum DeviceType {
    AirPods,
}

impl Display for DeviceType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeviceType::AirPods => write!(f, "AirPods"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data")]
pub enum DeviceInformation {
    AirPods(AirPodsInformation),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceData {
    pub name: String,
    pub type_: DeviceType,
    pub information: Option<DeviceInformation>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AirPodsNoiseControlMode {
    Off,
    NoiseCancellation,
    Transparency,
    Adaptive,
}

impl Display for AirPodsNoiseControlMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AirPodsNoiseControlMode::Off => write!(f, "Off"),
            AirPodsNoiseControlMode::NoiseCancellation => write!(f, "Noise Cancellation"),
            AirPodsNoiseControlMode::Transparency => write!(f, "Transparency"),
            AirPodsNoiseControlMode::Adaptive => write!(f, "Adaptive"),
        }
    }
}

impl AirPodsNoiseControlMode {
    pub fn from_byte(value: u8) -> Self {
        match value {
            0x01 => AirPodsNoiseControlMode::Off,
            0x02 => AirPodsNoiseControlMode::NoiseCancellation,
            0x03 => AirPodsNoiseControlMode::Transparency,
            0x04 => AirPodsNoiseControlMode::Adaptive,
            _ => AirPodsNoiseControlMode::Off,
        }
    }
    pub fn to_byte(&self) -> u8 {
        match self {
            AirPodsNoiseControlMode::Off => 0x01,
            AirPodsNoiseControlMode::NoiseCancellation => 0x02,
            AirPodsNoiseControlMode::Transparency => 0x03,
            AirPodsNoiseControlMode::Adaptive => 0x04,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_mode_byte_roundtrip() {
        for m in [
            AirPodsNoiseControlMode::Off,
            AirPodsNoiseControlMode::NoiseCancellation,
            AirPodsNoiseControlMode::Transparency,
            AirPodsNoiseControlMode::Adaptive,
        ] {
            assert_eq!(AirPodsNoiseControlMode::from_byte(m.to_byte()), m);
        }
    }

    #[test]
    fn noise_mode_unknown_byte_falls_back_to_off() {
        assert_eq!(
            AirPodsNoiseControlMode::from_byte(0xFF),
            AirPodsNoiseControlMode::Off
        );
        assert_eq!(
            AirPodsNoiseControlMode::from_byte(0x00),
            AirPodsNoiseControlMode::Off
        );
    }

    #[test]
    fn noise_mode_display_human_readable() {
        assert_eq!(
            AirPodsNoiseControlMode::NoiseCancellation.to_string(),
            "Noise Cancellation"
        );
        assert_eq!(AirPodsNoiseControlMode::Adaptive.to_string(), "Adaptive");
        assert_eq!(AirPodsNoiseControlMode::Off.to_string(), "Off");
        assert_eq!(
            AirPodsNoiseControlMode::Transparency.to_string(),
            "Transparency"
        );
    }

    #[test]
    fn device_type_display() {
        assert_eq!(DeviceType::AirPods.to_string(), "AirPods");
    }
}
