use tao::event_loop::EventLoopProxy;

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum UiCommand {
    SetState { state: String, info: Option<String> },
    SetIsoMdlQr { uri: String, svg: String },
    ClientReady,
    Close,
}

#[allow(dead_code)]
#[derive(Clone)]
pub struct WebviewHandle {
    proxy: EventLoopProxy<UiCommand>,
}

impl WebviewHandle {
    pub fn new(proxy: EventLoopProxy<UiCommand>) -> Self {
        Self { proxy }
    }

    pub fn set_discovery(&self) {
        let _ = self.proxy.send_event(UiCommand::SetState {
            state: "standby".to_string(),
            info: None,
        });
    }

    pub fn set_connecting(&self) {
        let _ = self.proxy.send_event(UiCommand::SetState {
            state: "connecting".to_string(),
            info: None,
        });
    }

    pub fn set_access_granted(&self, resource: String) {
        let _ = self.proxy.send_event(UiCommand::SetState {
            state: "granted".to_string(),
            info: Some(resource),
        });
    }

    pub fn set_access_denied(&self, reason: String) {
        let _ = self.proxy.send_event(UiCommand::SetState {
            state: "denied".to_string(),
            info: Some(reason),
        });
    }

    #[allow(dead_code)]
    pub fn set_iso_mdl_qr(&self, uri: String, svg: String) {
        let _ = self.proxy.send_event(UiCommand::SetIsoMdlQr { uri, svg });
    }

    #[allow(dead_code)]
    pub fn close(&self) {
        let _ = self.proxy.send_event(UiCommand::Close);
    }
}

