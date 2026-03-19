use crate::bluetooth::aacp::AACPManager;
use std::sync::Arc;

pub struct DeviceManagers {
    aacp: Arc<AACPManager>,
}

impl DeviceManagers {
    pub fn with_aacp(aacp: AACPManager) -> Self {
        Self {
            aacp: Arc::new(aacp),
        }
    }

    pub fn set_aacp(&mut self, manager: AACPManager) {
        self.aacp = Arc::new(manager);
    }

    pub fn get_aacp(&self) -> Arc<AACPManager> {
        self.aacp.clone()
    }
}
