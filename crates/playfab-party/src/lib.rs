//! Minimal bindings to the PlayFab Party C API, loaded at runtime from
//! `libparty.so` so the crate builds and tests without it.
//!
//! Party is single-threaded from our side: create [`Party`] on one thread,
//! call [`Party::poll`] often (every few ms), and do all calls from there.
//! Only the data path is covered: local user, network join, one endpoint,
//! messages. No chat controls or audio.

pub mod ffi;

use std::ffi::{c_char, c_void, CStr, CString};
use std::path::Path;
use std::ptr;

use libloading::Library;

use ffi::*;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not load {path}: {source}")]
    Load {
        path: String,
        source: libloading::Error,
    },
    #[error("libparty is missing symbol {0}")]
    Symbol(&'static str),
    #[error("{call} failed: 0x{code:X} {message}")]
    Party {
        call: &'static str,
        code: u32,
        message: String,
    },
    #[error("string argument contains a NUL byte")]
    Nul,
    #[error("message too large")]
    TooLarge,
}

pub type Result<T> = std::result::Result<T, Error>;

macro_rules! handle_type {
    ($name:ident) => {
        /// Opaque Party handle. Only meaningful to the [`Party`] it came from.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub struct $name(usize);

        impl $name {
            fn raw(self) -> Handle {
                self.0 as Handle
            }
            fn from_raw(h: Handle) -> Self {
                Self(h as usize)
            }
        }
    };
}

handle_type!(LocalUser);
handle_type!(Network);
handle_type!(Endpoint);

/// `PARTY_STATE_CHANGE_RESULT`; 0 is success.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StateResult {
    pub result: u32,
    pub error_detail: u32,
}

impl StateResult {
    pub fn ok(&self) -> bool {
        self.result == 0
    }
}

impl std::fmt::Display for StateResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self.result {
            0 => "succeeded",
            1 => "unknown error",
            2 => "canceled by title",
            3 => "internet connectivity error",
            4 => "party service error",
            5 => "no servers available",
            6 => "user not authorized",
            7 => "user create network throttled",
            8 => "title not enabled for party",
            10 => "network limit reached",
            11 => "network no longer exists",
            12 => "network not joinable",
            13 => "version mismatch",
            14 => "leave network called",
            15 => "failed to bind to local UDP socket",
            _ => "other",
        };
        write!(
            f,
            "{name} (result {}, detail 0x{:X})",
            self.result, self.error_detail
        )
    }
}

/// The state changes we act on, copied out of Party's buffers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    ConnectToNetworkCompleted {
        result: StateResult,
        network: Network,
    },
    AuthenticateLocalUserCompleted {
        result: StateResult,
    },
    CreateEndpointCompleted {
        result: StateResult,
        endpoint: Endpoint,
    },
    EndpointCreated {
        endpoint: Endpoint,
        entity_id: Option<String>,
        is_local: bool,
    },
    EndpointDestroyed {
        endpoint: Endpoint,
        reason: u32,
    },
    MessageReceived {
        sender: Endpoint,
        data: Vec<u8>,
    },
    NetworkDestroyed {
        reason: u32,
        error_detail: u32,
    },
    LocalUserKicked,
    LocalUserRemoved {
        reason: u32,
    },
    LeaveNetworkCompleted,
    /// Any other state change, by `PARTY_STATE_CHANGE_TYPE`.
    Other(u32),
}

struct Api {
    _lib: Library,
    initialize: FnInitialize,
    cleanup: FnCleanup,
    get_error_message: FnGetErrorMessage,
    create_local_user: FnCreateLocalUser,
    local_user_update_entity_token: FnLocalUserUpdateEntityToken,
    deserialize_network_descriptor: FnDeserializeNetworkDescriptor,
    connect_to_network: FnConnectToNetwork,
    network_authenticate_local_user: FnNetworkAuthenticateLocalUser,
    network_create_endpoint: FnNetworkCreateEndpoint,
    network_leave_network: FnNetworkLeaveNetwork,
    endpoint_send_message: FnEndpointSendMessage,
    endpoint_get_entity_id: FnEndpointGetEntityId,
    endpoint_is_local: FnEndpointIsLocal,
    start_processing_state_changes: FnStartProcessingStateChanges,
    finish_processing_state_changes: FnFinishProcessingStateChanges,
}

impl Api {
    fn load(path: &Path) -> Result<Self> {
        // SAFETY: loading a shared library runs its initializers; the caller
        // chose to trust this libparty.so.
        let lib = unsafe { Library::new(path) }.map_err(|source| Error::Load {
            path: path.display().to_string(),
            source,
        })?;
        macro_rules! sym {
            ($name:literal) => {{
                // SAFETY: the type comes from Party_c.h for this symbol.
                let s = unsafe { lib.get(concat!($name, "\0").as_bytes()) }
                    .map_err(|_| Error::Symbol($name))?;
                *s
            }};
        }
        Ok(Self {
            initialize: sym!("PartyInitialize"),
            cleanup: sym!("PartyCleanup"),
            get_error_message: sym!("PartyGetErrorMessage"),
            create_local_user: sym!("PartyCreateLocalUser"),
            local_user_update_entity_token: sym!("PartyLocalUserUpdateEntityToken"),
            deserialize_network_descriptor: sym!("PartyDeserializeNetworkDescriptor"),
            connect_to_network: sym!("PartyConnectToNetwork"),
            network_authenticate_local_user: sym!("PartyNetworkAuthenticateLocalUser"),
            network_create_endpoint: sym!("PartyNetworkCreateEndpoint"),
            network_leave_network: sym!("PartyNetworkLeaveNetwork"),
            endpoint_send_message: sym!("PartyEndpointSendMessage"),
            endpoint_get_entity_id: sym!("PartyEndpointGetEntityId"),
            endpoint_is_local: sym!("PartyEndpointIsLocal"),
            start_processing_state_changes: sym!("PartyStartProcessingStateChanges"),
            finish_processing_state_changes: sym!("PartyFinishProcessingStateChanges"),
            _lib: lib,
        })
    }

    fn check(&self, call: &'static str, code: PartyError) -> Result<()> {
        if code == 0 {
            return Ok(());
        }
        let mut msg: PartyString = ptr::null();
        // SAFETY: valid out pointer; Party returns a static string.
        let message = if unsafe { (self.get_error_message)(code, &mut msg) } == 0 && !msg.is_null()
        {
            unsafe { CStr::from_ptr(msg) }
                .to_string_lossy()
                .into_owned()
        } else {
            String::new()
        };
        Err(Error::Party {
            call,
            code,
            message,
        })
    }
}

fn cstring(s: &str) -> Result<CString> {
    CString::new(s).map_err(|_| Error::Nul)
}

unsafe fn opt_string(p: *const c_char) -> Option<String> {
    (!p.is_null()).then(|| CStr::from_ptr(p).to_string_lossy().into_owned())
}

/// An initialized Party instance. Dropping it calls `PartyCleanup`.
pub struct Party {
    api: Api,
    handle: Handle,
}

impl Party {
    /// Loads `libparty.so` from `lib_path` and calls `PartyInitialize`.
    pub fn initialize(lib_path: &Path, title_id: &str) -> Result<Self> {
        let api = Api::load(lib_path)?;
        let title = cstring(title_id)?;
        let mut handle: Handle = ptr::null();
        // SAFETY: valid C string and out pointer.
        let rc = unsafe { (api.initialize)(title.as_ptr(), &mut handle) };
        api.check("PartyInitialize", rc)?;
        Ok(Self { api, handle })
    }

    pub fn create_local_user(&self, entity_id: &str, entity_token: &str) -> Result<LocalUser> {
        let id = cstring(entity_id)?;
        let tok = cstring(entity_token)?;
        let mut user: Handle = ptr::null();
        // SAFETY: valid handle, strings and out pointer.
        let rc = unsafe {
            (self.api.create_local_user)(self.handle, id.as_ptr(), tok.as_ptr(), &mut user)
        };
        self.api.check("PartyCreateLocalUser", rc)?;
        Ok(LocalUser::from_raw(user))
    }

    pub fn update_entity_token(&self, user: LocalUser, entity_token: &str) -> Result<()> {
        let tok = cstring(entity_token)?;
        // SAFETY: handle from create_local_user.
        let rc = unsafe { (self.api.local_user_update_entity_token)(user.raw(), tok.as_ptr()) };
        self.api.check("PartyLocalUserUpdateEntityToken", rc)
    }

    /// Deserializes a network descriptor string and starts joining it.
    /// Completion arrives as [`Event::ConnectToNetworkCompleted`].
    pub fn connect_to_network(&self, serialized_descriptor: &str) -> Result<Network> {
        let s = cstring(serialized_descriptor)?;
        // SAFETY: plain-data struct, fully written by Party on success.
        let mut desc: PARTY_NETWORK_DESCRIPTOR = unsafe { std::mem::zeroed() };
        let rc = unsafe { (self.api.deserialize_network_descriptor)(s.as_ptr(), &mut desc) };
        self.api.check("PartyDeserializeNetworkDescriptor", rc)?;
        let mut net: Handle = ptr::null();
        // SAFETY: descriptor lives across the call; Party copies it.
        let rc =
            unsafe { (self.api.connect_to_network)(self.handle, &desc, ptr::null_mut(), &mut net) };
        self.api.check("PartyConnectToNetwork", rc)?;
        Ok(Network::from_raw(net))
    }

    /// Completion arrives as [`Event::AuthenticateLocalUserCompleted`].
    pub fn authenticate_local_user(
        &self,
        network: Network,
        user: LocalUser,
        invitation_id: &str,
    ) -> Result<()> {
        let inv = cstring(invitation_id)?;
        // SAFETY: handles from this Party; Party copies the string.
        let rc = unsafe {
            (self.api.network_authenticate_local_user)(
                network.raw(),
                user.raw(),
                inv.as_ptr(),
                ptr::null_mut(),
            )
        };
        self.api.check("PartyNetworkAuthenticateLocalUser", rc)
    }

    /// Completion arrives as [`Event::CreateEndpointCompleted`].
    pub fn create_endpoint(&self, network: Network, user: LocalUser) -> Result<Endpoint> {
        let mut ep: Handle = ptr::null();
        // SAFETY: no shared properties; handles from this Party.
        let rc = unsafe {
            (self.api.network_create_endpoint)(
                network.raw(),
                user.raw(),
                0,
                ptr::null(),
                ptr::null(),
                ptr::null_mut(),
                &mut ep,
            )
        };
        self.api.check("PartyNetworkCreateEndpoint", rc)?;
        Ok(Endpoint::from_raw(ep))
    }

    /// Sends `data` from our endpoint to one target, guaranteed and in order.
    /// Party copies the buffer.
    pub fn send(&self, from: Endpoint, to: Endpoint, data: &[u8]) -> Result<()> {
        let len = u32::try_from(data.len()).map_err(|_| Error::TooLarge)?;
        let target = to.raw();
        let buf = PARTY_DATA_BUFFER {
            buffer: data.as_ptr() as *const c_void,
            buffer_byte_count: len,
        };
        // SAFETY: one target and one buffer, both valid for the call.
        let rc = unsafe {
            (self.api.endpoint_send_message)(
                from.raw(),
                1,
                &target,
                SEND_GUARANTEED_DELIVERY | SEND_SEQUENTIAL_DELIVERY,
                ptr::null(),
                1,
                &buf,
                ptr::null_mut(),
            )
        };
        self.api.check("PartyEndpointSendMessage", rc)
    }

    pub fn leave_network(&self, network: Network) -> Result<()> {
        // SAFETY: handle from this Party.
        let rc = unsafe { (self.api.network_leave_network)(network.raw(), ptr::null_mut()) };
        self.api.check("PartyNetworkLeaveNetwork", rc)
    }

    pub fn endpoint_entity_id(&self, endpoint: Endpoint) -> Option<String> {
        let mut s: PartyString = ptr::null();
        // SAFETY: handle from a state change; string owned by Party.
        let rc = unsafe { (self.api.endpoint_get_entity_id)(endpoint.raw(), &mut s) };
        if rc != 0 {
            return None;
        }
        unsafe { opt_string(s) }
    }

    pub fn endpoint_is_local(&self, endpoint: Endpoint) -> bool {
        let mut b: PartyBool = 0;
        // SAFETY: handle from a state change.
        let rc = unsafe { (self.api.endpoint_is_local)(endpoint.raw(), &mut b) };
        rc == 0 && b != 0
    }

    /// Drains pending state changes. Call this often; Party does its
    /// networking on its own threads but only reports here.
    pub fn poll(&self) -> Result<Vec<Event>> {
        let mut count = 0u32;
        let mut changes: *const *const PARTY_STATE_CHANGE = ptr::null();
        // SAFETY: valid out pointers.
        let rc = unsafe {
            (self.api.start_processing_state_changes)(self.handle, &mut count, &mut changes)
        };
        self.api.check("PartyStartProcessingStateChanges", rc)?;
        let mut events = Vec::with_capacity(count as usize);
        for i in 0..count as usize {
            // SAFETY: Party hands us `count` valid pointers until Finish.
            let sc = unsafe { *changes.add(i) };
            events.push(unsafe { self.convert(sc) });
        }
        // SAFETY: returning exactly what Start gave us.
        let rc = unsafe { (self.api.finish_processing_state_changes)(self.handle, count, changes) };
        self.api.check("PartyFinishProcessingStateChanges", rc)?;
        Ok(events)
    }

    unsafe fn convert(&self, sc: *const PARTY_STATE_CHANGE) -> Event {
        let kind = (*sc).state_change_type;
        tracing::trace!(kind, "party state change");
        match kind {
            SC_CONNECT_TO_NETWORK_COMPLETED => {
                let c = &*(sc as *const PARTY_CONNECT_TO_NETWORK_COMPLETED_STATE_CHANGE);
                Event::ConnectToNetworkCompleted {
                    result: StateResult {
                        result: c.result,
                        error_detail: c.error_detail,
                    },
                    network: Network::from_raw(c.network),
                }
            }
            SC_AUTHENTICATE_LOCAL_USER_COMPLETED => {
                let c = &*(sc as *const PARTY_AUTHENTICATE_LOCAL_USER_COMPLETED_STATE_CHANGE);
                Event::AuthenticateLocalUserCompleted {
                    result: StateResult {
                        result: c.result,
                        error_detail: c.error_detail,
                    },
                }
            }
            SC_CREATE_ENDPOINT_COMPLETED => {
                let c = &*(sc as *const PARTY_CREATE_ENDPOINT_COMPLETED_STATE_CHANGE);
                Event::CreateEndpointCompleted {
                    result: StateResult {
                        result: c.result,
                        error_detail: c.error_detail,
                    },
                    endpoint: Endpoint::from_raw(c.local_endpoint),
                }
            }
            SC_ENDPOINT_CREATED => {
                let c = &*(sc as *const PARTY_ENDPOINT_CREATED_STATE_CHANGE);
                let endpoint = Endpoint::from_raw(c.endpoint);
                Event::EndpointCreated {
                    endpoint,
                    entity_id: self.endpoint_entity_id(endpoint),
                    is_local: self.endpoint_is_local(endpoint),
                }
            }
            SC_ENDPOINT_DESTROYED => {
                let c = &*(sc as *const PARTY_ENDPOINT_DESTROYED_STATE_CHANGE);
                Event::EndpointDestroyed {
                    endpoint: Endpoint::from_raw(c.endpoint),
                    reason: c.reason,
                }
            }
            SC_ENDPOINT_MESSAGE_RECEIVED => {
                let c = &*(sc as *const PARTY_ENDPOINT_MESSAGE_RECEIVED_STATE_CHANGE);
                let data = if c.message_buffer.is_null() || c.message_size == 0 {
                    Vec::new()
                } else {
                    std::slice::from_raw_parts(
                        c.message_buffer as *const u8,
                        c.message_size as usize,
                    )
                    .to_vec()
                };
                Event::MessageReceived {
                    sender: Endpoint::from_raw(c.sender_endpoint),
                    data,
                }
            }
            SC_NETWORK_DESTROYED => {
                let c = &*(sc as *const PARTY_NETWORK_DESTROYED_STATE_CHANGE);
                Event::NetworkDestroyed {
                    reason: c.reason,
                    error_detail: c.error_detail,
                }
            }
            SC_LOCAL_USER_KICKED => Event::LocalUserKicked,
            SC_LOCAL_USER_REMOVED => {
                let c = &*(sc as *const PARTY_LOCAL_USER_REMOVED_STATE_CHANGE);
                Event::LocalUserRemoved {
                    reason: c.removed_reason,
                }
            }
            SC_LEAVE_NETWORK_COMPLETED => Event::LeaveNetworkCompleted,
            other => Event::Other(other),
        }
    }
}

impl Drop for Party {
    fn drop(&mut self) {
        // SAFETY: handle from PartyInitialize, cleaned up once.
        let rc = unsafe { (self.api.cleanup)(self.handle) };
        if rc != 0 {
            tracing::warn!(code = rc, "PartyCleanup failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_library_is_a_load_error() {
        let err = Party::initialize(Path::new("/nonexistent/libparty.so"), "6E223")
            .err()
            .unwrap();
        assert!(matches!(err, Error::Load { .. }), "{err}");
    }
}
