use super::files::FileList;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offer {
    pub version: u64,
    pub files: bool,
    pub local: Option<u64>,
}
#[derive(Clone, Debug)]
pub enum Incoming {
    Local(u64),
    External {
        stamp: (u64, u64),
        files: Option<FileList>,
    },
}
#[derive(Clone, Debug)]
pub enum Payload {
    Files(FileList),
    Text(String),
}
#[derive(Clone, Debug)]
pub enum Request {
    Write {
        id: u64,
        generation: u64,
        payload: Payload,
        expected: Option<u64>,
    },
    Read {
        id: u64,
    },
    ReadText {
        id: u64,
    },
}
#[derive(Debug)]
pub enum Event {
    Ready(Result<(), String>),
    Offer(Offer),
    Written {
        id: u64,
        result: Result<(), String>,
    },
    Read {
        id: u64,
        result: Result<Incoming, String>,
    },
    Text {
        id: u64,
        result: Result<String, String>,
    },
}
