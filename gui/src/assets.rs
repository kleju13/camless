//! The page, built into the program. `--ui-dir` reads the same files from a folder
//! instead; nothing outside this list is served either way.

use std::path::Path;

const HTML: &str = "text/html; charset=utf-8";
const JS: &str = "text/javascript; charset=utf-8";
const CSS: &str = "text/css; charset=utf-8";

const FILES: &[(&str, &str, &[u8])] = &[
    ("index.html", HTML, include_bytes!("../ui/index.html")),
    ("app.css", CSS, include_bytes!("../ui/app.css")),
    ("store.js", JS, include_bytes!("../ui/store.js")),
    ("kit.js", JS, include_bytes!("../ui/kit.js")),
    ("kraken-draw.js", JS, include_bytes!("../ui/kraken-draw.js")),
    ("curve-editor.js", JS, include_bytes!("../ui/curve-editor.js")),
    ("app.js", JS, include_bytes!("../ui/app.js")),
    ("montserrat.woff2", "font/woff2", include_bytes!("../ui/montserrat.woff2")),
];

/// The file for a request path, and its content type.
pub fn get(path: &str, dir: Option<&Path>) -> Option<(&'static str, Vec<u8>)> {
    let name = if path == "/" { "index.html" } else { path.strip_prefix('/')? };
    let &(_, content_type, built_in) = FILES.iter().find(|(n, ..)| *n == name)?;
    match dir {
        Some(d) => std::fs::read(d.join(name)).ok().map(|b| (content_type, b)),
        None => Some((content_type, built_in.to_vec())),
    }
}
