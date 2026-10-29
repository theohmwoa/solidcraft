//! Promise-based access to the running app's existing control channel.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::Duration;

use serde_json::{Value, json};
use solidcraft_engine::io::vfs;
use solidcraft_ui_egui::ControlRequest;
use wasm_bindgen::{JsValue, closure::Closure};

use crate::disk;

const MAX_PENDING: usize = 256;
const MAX_JSON_BYTES: usize = 16 * 1024 * 1024;

struct Pending {
    reply: Receiver<Value>,
    resolve: js_sys::Function,
    reject: js_sys::Function,
}

#[derive(Clone)]
pub struct Api {
    sender: Sender<ControlRequest>,
    pending: Rc<RefCell<Vec<Pending>>>,
    ctx: egui::Context,
}

fn params(value: &JsValue) -> Result<Value, String> {
    if value.is_undefined() || value.is_null() {
        return Ok(json!({}));
    }
    let text = js_sys::JSON::stringify(value).ok().and_then(|v| v.as_string()).ok_or("params must be JSON serializable")?;
    if text.len() > MAX_JSON_BYTES {
        return Err("params exceed 16 MiB".into());
    }
    serde_json::from_str(&text).map_err(|e| format!("invalid params: {e}"))
}

fn error(message: &str) -> JsValue {
    js_sys::Error::new(message).into()
}

/// Host files stay in memory: the private storage, disk and download paths are only filled from
/// what commands write, which `files::flush` forwards, and a file inserted there would never be stored.
fn host_path(path: &str) -> Result<&str, String> {
    if disk::route(path) == disk::WriteTo::Memory {
        Ok(path)
    } else {
        Err(format!("{path}: readFile and writeFile take memory paths, not {}, {} or {} paths", disk::OPFS_PREFIX, disk::PREFIX, disk::DOWNLOADS_PREFIX))
    }
}

fn read_file(path: String) -> js_sys::Promise {
    match host_path(&path).and_then(|path| vfs::read(path, usize::MAX).map_err(|e| e.to_string())) {
        Ok(bytes) => js_sys::Promise::resolve(&JsValue::from(js_sys::Uint8Array::from(bytes.as_slice()))),
        Err(message) => js_sys::Promise::reject(&error(&message)),
    }
}

/// The bytes behind a typed array or `DataView`. A page that embeds the app passes arrays made in
/// its own window, which an `instanceof Uint8Array` check in this window rejects.
fn view_bytes(value: &JsValue) -> Option<Vec<u8>> {
    if !js_sys::ArrayBuffer::is_view(value) {
        return None;
    }
    let get = |name: &str| js_sys::Reflect::get(value, &name.into()).ok();
    let buffer = get("buffer")?;
    let offset = get("byteOffset")?.as_f64()? as u32;
    let length = get("byteLength")?.as_f64()? as u32;
    Some(js_sys::Uint8Array::new_with_byte_offset_and_length(&buffer, offset, length).to_vec())
}

fn write_file(path: String, bytes: JsValue) -> js_sys::Promise {
    let Some(bytes) = view_bytes(&bytes) else {
        return js_sys::Promise::reject(&error("writeFile takes the file's bytes as a Uint8Array"));
    };
    match host_path(&path) {
        Ok(path) => {
            vfs::insert(path, bytes);
            js_sys::Promise::resolve(&JsValue::UNDEFINED)
        }
        Err(message) => js_sys::Promise::reject(&error(&message)),
    }
}

impl Api {
    pub fn install(sender: Sender<ControlRequest>, ctx: egui::Context) -> Result<Self, JsValue> {
        let api = Self { sender, pending: Rc::default(), ctx };
        let object = js_sys::Object::new();
        let request = api.clone();
        let execute = api.clone();
        let commands = api.clone();
        let inspect = api.clone();
        for (name, function) in [
            (
                "request",
                Closure::<dyn Fn(String, JsValue) -> js_sys::Promise>::new(move |method: String, value: JsValue| {
                    request.request(&method, params(&value))
                })
                .into_js_value(),
            ),
            (
                "execute",
                Closure::<dyn Fn(String, JsValue) -> js_sys::Promise>::new(move |command: String, value: JsValue| {
                    execute.request("engine.execute", params(&value).map(|p| json!({"command": command, "params": p})))
                })
                .into_js_value(),
            ),
            ("commands", Closure::<dyn Fn() -> js_sys::Promise>::new(move || commands.request("engine.commands", Ok(json!({})))).into_js_value()),
            (
                "inspect",
                Closure::<dyn Fn(JsValue) -> js_sys::Promise>::new(move |value: JsValue| inspect.request("document.inspect", params(&value)))
                    .into_js_value(),
            ),
            ("readFile", Closure::<dyn Fn(String) -> js_sys::Promise>::new(read_file).into_js_value()),
            ("writeFile", Closure::<dyn Fn(String, JsValue) -> js_sys::Promise>::new(write_file).into_js_value()),
        ] {
            js_sys::Reflect::set(&object, &name.into(), &function)?;
        }
        let window = web_sys::window().ok_or_else(|| error("no window"))?;
        js_sys::Reflect::set(&window, &"solidcraft".into(), &object)?;
        Ok(api)
    }

    fn request(&self, method: &str, params: Result<Value, String>) -> js_sys::Promise {
        let params = match params {
            Ok(params) => params,
            Err(message) => return js_sys::Promise::reject(&error(&message)),
        };
        if self.pending.borrow().len() >= MAX_PENDING {
            return js_sys::Promise::reject(&error("too many pending SolidCraft requests (maximum 256)"));
        }
        js_sys::Promise::new(&mut |resolve, reject| {
            let (request, reply) = ControlRequest::new(method, params.clone());
            if self.sender.send(request).is_err() {
                let _ = reject.call1(&JsValue::UNDEFINED, &error("SolidCraft is not running"));
                return;
            }
            self.pending.borrow_mut().push(Pending { reply, resolve, reject });
            self.ctx.request_repaint();
        })
    }

    pub fn poll(&self) {
        let mut completed = Vec::new();
        self.pending.borrow_mut().retain_mut(|pending| {
            let result = match pending.reply.try_recv() {
                Ok(reply) => {
                    if reply.get("ok").and_then(Value::as_bool) == Some(true) {
                        js_sys::JSON::parse(&reply.get("result").unwrap_or(&Value::Null).to_string())
                    } else {
                        Err(error(reply.get("error").and_then(Value::as_str).unwrap_or("SolidCraft request failed")))
                    }
                }
                Err(TryRecvError::Empty) => return true,
                Err(TryRecvError::Disconnected) => Err(error("SolidCraft request dropped")),
            };
            completed.push((pending.resolve.clone(), pending.reject.clone(), result));
            false
        });
        // Release the queue before calling JavaScript, which can submit another request.
        for (resolve, reject, result) in completed {
            match result {
                Ok(value) => {
                    let _ = resolve.call1(&JsValue::UNDEFINED, &value);
                }
                Err(error) => {
                    let _ = reject.call1(&JsValue::UNDEFINED, &error);
                }
            }
        }
        if !self.pending.borrow().is_empty() {
            self.ctx.request_repaint_after(Duration::from_millis(16));
        }
    }
}
