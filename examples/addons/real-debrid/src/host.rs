//! The SDK's guest-side host helpers, under the name the add-on uses.

pub use rvah_addon_sdk::guest::{
    form_encode, http_get, http_post_form, http_send, log, secret_get, storage_get, storage_set,
    Host,
};
