pub mod webview;

pub const TERMINAL_HTML_TEMPLATE: &str = include_str!("terminal.html");

pub fn render_terminal_html(qr_uri: &str, qr_svg: &str) -> String {
    TERMINAL_HTML_TEMPLATE
        .replace("<!-- QR_SVG_CONTENT -->", qr_svg)
        .replace("<!-- QR_URI_CONTENT -->", qr_uri)
}
