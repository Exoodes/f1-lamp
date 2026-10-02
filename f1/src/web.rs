use esp_idf_svc::http::server::{Configuration, EspHttpServer};
use esp_idf_svc::http::Method;
use esp_idf_svc::io::Write;

const INDEX_HTML: &str = include_str!("../web/index.html");
const STACK_SIZE: usize = 10 * 1024;

pub fn start() -> anyhow::Result<EspHttpServer<'static>> {
    let config = Configuration {
        stack_size: STACK_SIZE,
        ..Default::default()
    };

    let mut server = EspHttpServer::new(&config)?;

    server.fn_handler("/", Method::Get, |req| {
        req.into_response(200, None, &[("Content-Type", "text/html; charset=utf-8")])?
            .write_all(INDEX_HTML.as_bytes())
    })?;
    log::info!("web server started");
    Ok(server)
}
