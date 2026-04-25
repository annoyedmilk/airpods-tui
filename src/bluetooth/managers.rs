use crate::bluetooth::aacp::AACPManager;
use std::sync::Arc;

pub struct DeviceManagers {
    aacp: Option<Arc<AACPManager>>,
}

impl DeviceManagers {
    /// Reserve a HashMap slot before async init starts so concurrent
    /// connection events can detect the in-progress claim.
    pub fn placeholder() -> Self {
        Self { aacp: None }
    }

    pub fn with_aacp(aacp: AACPManager) -> Self {
        Self {
            aacp: Some(Arc::new(aacp)),
        }
    }

    pub fn set_aacp(&mut self, manager: AACPManager) {
        self.aacp = Some(Arc::new(manager));
    }

    pub fn get_aacp(&self) -> Option<Arc<AACPManager>> {
        self.aacp.clone()
    }
}
