//! Host-only, memory-only relay cryptography. JavaScript owns timers and routes.
use rvah_stream_relay::{DeliveryReceipt, EncryptedFrame, SessionId, SessionKey};
use wasm_bindgen::prelude::*;

fn error(value: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&value.to_string())
}

#[wasm_bindgen]
pub struct BrowserRelaySegment {
    frames: Vec<EncryptedFrame>,
    key: SessionKey,
    session: SessionId,
    length: u64,
    digest: String,
}

#[wasm_bindgen]
impl BrowserRelaySegment {
    #[wasm_bindgen(constructor)]
    pub fn new(session: &str, key: &[u8], bytes: &[u8]) -> Result<Self, JsValue> {
        let key = SessionKey::from_bytes(key.try_into().map_err(error)?);
        let session = SessionId::parse(session).map_err(error)?;
        let frames = rvah_stream_relay::encrypt_segment(&session, 0, bytes, &key).map_err(error)?;
        Ok(Self {
            length: frames[0].segment_byte_length,
            digest: frames[0].segment_sha256.clone(),
            frames,
            key,
            session,
        })
    }

    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    pub fn digest(&self) -> String {
        self.digest.clone()
    }

    pub fn frame(&self, index: usize) -> Result<String, JsValue> {
        let frame = self
            .frames
            .get(index)
            .ok_or_else(|| error("Relay segment expired or frame missing"))?;
        serde_json::to_string(frame).map_err(error)
    }

    /// Idempotent so a lost receipt reply can safely be retried.
    pub fn acknowledge(&mut self, receipt: &str) -> Result<(), JsValue> {
        let receipt: DeliveryReceipt = serde_json::from_str(receipt).map_err(error)?;
        rvah_stream_relay::verify_receipt(&receipt, &self.key).map_err(error)?;
        if receipt.session_id != self.session
            || receipt.segment != 0
            || receipt.byte_length != self.length
            || receipt.plaintext_sha256 != self.digest
        {
            return Err(error("Receipt does not match this segment"));
        }
        self.frames.clear();
        Ok(())
    }
}

#[wasm_bindgen]
pub fn receive_relay_segment(frames: &str, key: &[u8]) -> Result<JsValue, JsValue> {
    // The wire receiver enforces compact framing limits before constructing JSON.
    if frames.len() > 80 * 1024 * 1024 {
        return Err(error("Relay frames exceed the input bound"));
    }
    let frames: Vec<EncryptedFrame> = serde_json::from_str(frames).map_err(error)?;
    let key = SessionKey::from_bytes(key.try_into().map_err(error)?);
    let received = rvah_stream_relay::receive_segment(&frames, &key).map_err(error)?;
    let result = js_sys::Object::new();
    js_sys::Reflect::set(
        &result,
        &"bytes".into(),
        &js_sys::Uint8Array::from(received.bytes.as_slice()),
    )?;
    js_sys::Reflect::set(
        &result,
        &"receipt".into(),
        &serde_json::to_string(&received.receipt)
            .map_err(error)?
            .into(),
    )?;
    Ok(result.into())
}
