//! Download control through the installed clients' own local interfaces.
mod context;
mod manager;
mod peer;
mod transport;
pub use manager::Manager;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Client {
    Ea,
    Ubisoft,
}

impl Client {
    pub fn store(self) -> &'static str {
        match self {
            Self::Ea => "ea",
            Self::Ubisoft => "ubisoft",
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Ea => "EA app",
            Self::Ubisoft => "Ubisoft Connect",
        }
    }
    fn executable(self) -> &'static str {
        match self {
            Self::Ea => "EADesktop.exe",
            Self::Ubisoft => "upc.exe",
        }
    }
    #[cfg(windows)]
    fn helper(self) -> &'static str {
        match self {
            Self::Ea => "EACefSubProcess.exe",
            Self::Ubisoft => "UplayWebCore.exe",
        }
    }
}
