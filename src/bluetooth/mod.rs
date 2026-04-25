pub mod aacp;
pub(crate) mod discovery;
pub mod managers;

/// AACP service UUID used by AirPods for battery/settings communication.
pub const AIRPODS_AACP_UUID: &str = "74ec2172-0bad-4d01-8f77-997b2be0722a";
