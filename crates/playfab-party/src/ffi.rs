//! Raw declarations from `Party_c.h` (PlayFab Party 1.10 headers), limited to
//! what a data-only client needs. `PARTY_API` is `__stdcall`, which is the
//! plain C ABI on x86_64 Linux. Structs use `#pragma pack(8)`, which matches
//! `repr(C)` here because nothing is aligned above 8.

#![allow(non_camel_case_types, dead_code)]

use std::ffi::{c_char, c_void};

pub type PartyError = u32;
pub type PartyBool = u8;
pub type PartyString = *const c_char;
pub type Handle = *const c_void;

pub const NETWORK_IDENTIFIER_STRING_LENGTH: usize = 36;
pub const MAX_REGION_NAME_STRING_LENGTH: usize = 19;
pub const OPAQUE_CONNECTION_INFORMATION_BYTE_COUNT: usize = 300;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PARTY_NETWORK_DESCRIPTOR {
    pub network_identifier: [c_char; NETWORK_IDENTIFIER_STRING_LENGTH + 1],
    pub region_name: [c_char; MAX_REGION_NAME_STRING_LENGTH + 1],
    pub opaque_connection_information: [u8; OPAQUE_CONNECTION_INFORMATION_BYTE_COUNT],
}

#[repr(C)]
pub struct PARTY_DATA_BUFFER {
    pub buffer: *const c_void,
    pub buffer_byte_count: u32,
}

pub const SEND_GUARANTEED_DELIVERY: u32 = 0x1;
pub const SEND_SEQUENTIAL_DELIVERY: u32 = 0x2;

// PARTY_STATE_CHANGE_TYPE values used here.
pub const SC_REGIONS_CHANGED: u32 = 0;
pub const SC_CONNECT_TO_NETWORK_COMPLETED: u32 = 3;
pub const SC_AUTHENTICATE_LOCAL_USER_COMPLETED: u32 = 4;
pub const SC_LOCAL_USER_REMOVED: u32 = 7;
pub const SC_LOCAL_USER_KICKED: u32 = 9;
pub const SC_CREATE_ENDPOINT_COMPLETED: u32 = 10;
pub const SC_ENDPOINT_CREATED: u32 = 12;
pub const SC_ENDPOINT_DESTROYED: u32 = 13;
pub const SC_LEAVE_NETWORK_COMPLETED: u32 = 19;
pub const SC_NETWORK_DESTROYED: u32 = 20;
pub const SC_ENDPOINT_MESSAGE_RECEIVED: u32 = 21;

#[repr(C)]
pub struct PARTY_STATE_CHANGE {
    pub state_change_type: u32,
}

#[repr(C)]
pub struct PARTY_CONNECT_TO_NETWORK_COMPLETED_STATE_CHANGE {
    pub state_change: PARTY_STATE_CHANGE,
    pub result: u32,
    pub error_detail: PartyError,
    pub network_descriptor: PARTY_NETWORK_DESCRIPTOR,
    pub async_identifier: *mut c_void,
    pub network: Handle,
}

#[repr(C)]
pub struct PARTY_AUTHENTICATE_LOCAL_USER_COMPLETED_STATE_CHANGE {
    pub state_change: PARTY_STATE_CHANGE,
    pub result: u32,
    pub error_detail: PartyError,
    pub network: Handle,
    pub local_user: Handle,
    pub invitation_identifier: PartyString,
    pub async_identifier: *mut c_void,
}

#[repr(C)]
pub struct PARTY_LOCAL_USER_REMOVED_STATE_CHANGE {
    pub state_change: PARTY_STATE_CHANGE,
    pub network: Handle,
    pub local_user: Handle,
    pub removed_reason: u32,
}

#[repr(C)]
pub struct PARTY_CREATE_ENDPOINT_COMPLETED_STATE_CHANGE {
    pub state_change: PARTY_STATE_CHANGE,
    pub result: u32,
    pub error_detail: PartyError,
    pub network: Handle,
    pub local_user: Handle,
    pub async_identifier: *mut c_void,
    pub local_endpoint: Handle,
}

#[repr(C)]
pub struct PARTY_ENDPOINT_CREATED_STATE_CHANGE {
    pub state_change: PARTY_STATE_CHANGE,
    pub network: Handle,
    pub endpoint: Handle,
}

#[repr(C)]
pub struct PARTY_ENDPOINT_DESTROYED_STATE_CHANGE {
    pub state_change: PARTY_STATE_CHANGE,
    pub network: Handle,
    pub endpoint: Handle,
    pub reason: u32,
    pub error_detail: PartyError,
}

#[repr(C)]
pub struct PARTY_NETWORK_DESTROYED_STATE_CHANGE {
    pub state_change: PARTY_STATE_CHANGE,
    pub reason: u32,
    pub error_detail: PartyError,
    pub network: Handle,
}

#[repr(C)]
pub struct PARTY_ENDPOINT_MESSAGE_RECEIVED_STATE_CHANGE {
    pub state_change: PARTY_STATE_CHANGE,
    pub network: Handle,
    pub sender_endpoint: Handle,
    pub receiver_endpoint_count: u32,
    pub receiver_endpoints: *mut Handle,
    pub options: u32,
    pub message_size: u32,
    pub message_buffer: *const c_void,
}

pub type FnInitialize = unsafe extern "C" fn(PartyString, *mut Handle) -> PartyError;
pub type FnCleanup = unsafe extern "C" fn(Handle) -> PartyError;
pub type FnGetErrorMessage = unsafe extern "C" fn(PartyError, *mut PartyString) -> PartyError;
pub type FnCreateLocalUser =
    unsafe extern "C" fn(Handle, PartyString, PartyString, *mut Handle) -> PartyError;
pub type FnLocalUserUpdateEntityToken = unsafe extern "C" fn(Handle, PartyString) -> PartyError;
pub type FnDeserializeNetworkDescriptor =
    unsafe extern "C" fn(PartyString, *mut PARTY_NETWORK_DESCRIPTOR) -> PartyError;
pub type FnConnectToNetwork = unsafe extern "C" fn(
    Handle,
    *const PARTY_NETWORK_DESCRIPTOR,
    *mut c_void,
    *mut Handle,
) -> PartyError;
pub type FnNetworkAuthenticateLocalUser =
    unsafe extern "C" fn(Handle, Handle, PartyString, *mut c_void) -> PartyError;
pub type FnNetworkCreateEndpoint = unsafe extern "C" fn(
    Handle,
    Handle,
    u32,
    *const PartyString,
    *const PARTY_DATA_BUFFER,
    *mut c_void,
    *mut Handle,
) -> PartyError;
pub type FnNetworkLeaveNetwork = unsafe extern "C" fn(Handle, *mut c_void) -> PartyError;
pub type FnEndpointSendMessage = unsafe extern "C" fn(
    Handle,
    u32,
    *const Handle,
    u32,
    *const c_void,
    u32,
    *const PARTY_DATA_BUFFER,
    *mut c_void,
) -> PartyError;
pub type FnEndpointGetEntityId = unsafe extern "C" fn(Handle, *mut PartyString) -> PartyError;
pub type FnEndpointIsLocal = unsafe extern "C" fn(Handle, *mut PartyBool) -> PartyError;
pub type FnStartProcessingStateChanges =
    unsafe extern "C" fn(Handle, *mut u32, *mut *const *const PARTY_STATE_CHANGE) -> PartyError;
pub type FnFinishProcessingStateChanges =
    unsafe extern "C" fn(Handle, u32, *const *const PARTY_STATE_CHANGE) -> PartyError;

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{offset_of, size_of};

    /// Offsets printed by gcc for the same structs from `Party_c.h` on x86_64.
    #[test]
    fn layouts_match_header() {
        type Connect = PARTY_CONNECT_TO_NETWORK_COMPLETED_STATE_CHANGE;
        type Msg = PARTY_ENDPOINT_MESSAGE_RECEIVED_STATE_CHANGE;
        assert_eq!(size_of::<PARTY_NETWORK_DESCRIPTOR>(), 357);
        assert_eq!(offset_of!(Connect, async_identifier), 376);
        assert_eq!(offset_of!(Connect, network), 384);
        assert_eq!(offset_of!(Msg, message_size), 44);
        assert_eq!(offset_of!(Msg, message_buffer), 48);
        assert_eq!(
            offset_of!(PARTY_CREATE_ENDPOINT_COMPLETED_STATE_CHANGE, local_endpoint),
            40
        );
        assert_eq!(
            offset_of!(
                PARTY_AUTHENTICATE_LOCAL_USER_COMPLETED_STATE_CHANGE,
                async_identifier
            ),
            40
        );
        assert_eq!(
            offset_of!(PARTY_ENDPOINT_DESTROYED_STATE_CHANGE, error_detail),
            28
        );
        assert_eq!(
            offset_of!(PARTY_NETWORK_DESTROYED_STATE_CHANGE, network),
            16
        );
        assert_eq!(size_of::<PARTY_DATA_BUFFER>(), 16);
    }
}
