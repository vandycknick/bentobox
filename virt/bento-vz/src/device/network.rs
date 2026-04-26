use objc2::{rc::Retained, ClassType};
use objc2_virtualization::{
    VZNATNetworkDeviceAttachment, VZNetworkDeviceConfiguration, VZVirtioNetworkDeviceConfiguration,
};

#[derive(Debug, Clone)]
pub struct NetworkDeviceConfiguration {
    inner: Retained<VZVirtioNetworkDeviceConfiguration>,
}

impl NetworkDeviceConfiguration {
    pub fn nat() -> Self {
        unsafe {
            let inner = VZVirtioNetworkDeviceConfiguration::new();
            let attachment = VZNATNetworkDeviceAttachment::new();
            inner.setAttachment(Some(attachment.as_super()));
            Self { inner }
        }
    }

    pub(crate) fn as_inner(&self) -> &VZNetworkDeviceConfiguration {
        self.inner.as_super()
    }
}
