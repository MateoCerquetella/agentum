//! Images in a composer: pasted, dropped or picked, shown as thumbnails and sent as data: URLs
//! (shrunk to 1600 px on the long side: what the models look at anyway).
use agentum_ui::prelude::*;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

const MAX_SIDE: f64 = 1600.0;
const MAX_IMAGES: usize = 6;
/// A PNG bigger than this (as a data URL) goes as JPEG instead.
const MAX_PNG: usize = 2_500_000;

/// Add the image files among `files`.
pub fn add_files(files: Option<web_sys::FileList>, images: RwSignal<Vec<String>>) {
    let Some(files) = files else { return };
    for f in (0..files.length()).filter_map(|i| files.get(i)).filter(|f| f.type_().starts_with("image/")) {
        spawn_local(async move {
            match shrink(&f).await {
                Ok(url) if images.with_untracked(|v| v.len()) < MAX_IMAGES => {
                    let _ = images.try_update(|v| v.push(url));
                }
                Ok(_) => toast_bad(format!("At most {MAX_IMAGES} images a message.")),
                Err(e) => toast_bad(format!("Couldn't read {}: {e}", f.name())),
            }
        });
    }
}

fn has_files(dt: Option<web_sys::DataTransfer>) -> bool {
    dt.is_some_and(|d| d.types().iter().any(|t| t.as_string().as_deref() == Some("Files")))
}

/// For a composer's textarea: a pasted image (not text that also comes as a picture) is attached.
pub fn on_paste(e: web_sys::ClipboardEvent, images: RwSignal<Vec<String>>) {
    let Some(dt) = e.clipboard_data() else { return };
    if dt.get_data("text/plain").is_ok_and(|t| !t.is_empty()) {
        return;
    }
    if dt.files().is_some_and(|f| f.length() > 0) {
        e.prevent_default();
        add_files(dt.files(), images);
    }
}

/// For a composer's box: dragging files over it allows the drop.
pub fn on_dragover(e: web_sys::DragEvent) {
    if has_files(e.data_transfer()) {
        e.prevent_default();
    }
}

pub fn on_drop(e: web_sys::DragEvent, images: RwSignal<Vec<String>>) {
    if has_files(e.data_transfer()) {
        e.prevent_default();
        add_files(e.data_transfer().and_then(|d| d.files()), images);
    }
}

fn js(e: JsValue) -> String {
    e.as_string().or_else(|| e.dyn_ref::<js_sys::Error>().map(|e| e.message().into())).unwrap_or_else(|| "unreadable image".into())
}

async fn shrink(f: &web_sys::File) -> Result<String, String> {
    let win = web_sys::window().ok_or("no window")?;
    let bmp: web_sys::ImageBitmap = JsFuture::from(win.create_image_bitmap_with_blob(f).map_err(js)?).await.map_err(js)?.unchecked_into();
    let (w, h) = (bmp.width() as f64, bmp.height() as f64);
    let k = (MAX_SIDE / w.max(h)).min(1.0);
    let (cw, ch) = ((w * k).round().max(1.0), (h * k).round().max(1.0));
    let c: web_sys::HtmlCanvasElement = win.document().ok_or("no document")?.create_element("canvas").map_err(js)?.unchecked_into();
    c.set_width(cw as u32);
    c.set_height(ch as u32);
    let cx: web_sys::CanvasRenderingContext2d = c.get_context("2d").map_err(js)?.ok_or("no canvas")?.unchecked_into();
    cx.draw_image_with_image_bitmap_and_dw_and_dh(&bmp, 0.0, 0.0, cw, ch).map_err(js)?;
    bmp.close();
    // Screenshots stay PNG (sharp text) unless huge; photos go as JPEG.
    if matches!(f.type_().as_str(), "image/png" | "image/gif" | "image/webp") {
        let png = c.to_data_url().map_err(js)?;
        if png.len() <= MAX_PNG {
            return Ok(png);
        }
    }
    // JPEG has no transparency: white behind.
    let _ = cx.set_global_composite_operation("destination-over");
    cx.set_fill_style_str("#fff");
    cx.fill_rect(0.0, 0.0, cw, ch);
    c.to_data_url_with_type_and_encoder_options("image/jpeg", &JsValue::from_f64(0.85)).map_err(js)
}

/// The paperclip button and its hidden file input.
#[component]
pub fn AttachButton(images: RwSignal<Vec<String>>) -> impl IntoView {
    let input = NodeRef::<leptos::html::Input>::new();
    view! {
        <button type="button" class="cbox-icon" title="Attach images (or paste or drop them)" on:click=move |_| if let Some(i) = input.get() { i.click() }>
            <Icon name="paperclip" size=15/>
        </button>
        <input node_ref=input type="file" accept="image/*" multiple hidden
            on:change=move |e| { let t: web_sys::HtmlInputElement = event_target(&e); add_files(t.files(), images); t.set_value(""); }/>
    }
}

/// The attached images, each with a remove button.
#[component]
pub fn Thumbs(images: RwSignal<Vec<String>>) -> impl IntoView {
    view! {
        <Show when=move || images.with(|v| !v.is_empty())>
            <div class="cbox-thumbs">
                {move || images.get().into_iter().enumerate().map(|(i, src)| view! {
                    <div class="cbox-thumb">
                        <img src=src alt="Attached image"/>
                        <button type="button" class="cbox-thumb-x" title="Remove" on:click=move |_| images.update(|v| if i < v.len() { v.remove(i); })>
                            <Icon name="x" size=11/>
                        </button>
                    </div>
                }).collect_view()}
            </div>
        </Show>
    }
}
