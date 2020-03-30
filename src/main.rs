#[macro_use]
extern crate log;

use argh::FromArgs;
use bytes::{Bytes, BytesMut};
use env_logger::{Builder, Target};
use futures::SinkExt;
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::sync::{Arc, Mutex};
use tokio::net::{TcpListener, TcpStream};
use tokio::stream::StreamExt;
use tokio_util::codec::{Framed, LengthDelimitedCodec};

mod db;
mod parser;

// Need a better place to store these so they are searchable and not opaque to contributors
macro_rules! format_error_response {
    ($err:expr) => {{
        let mut out: BytesMut = BytesMut::from("!");
        out.extend_from_slice(&Bytes::from($err));
        out.into()
    }};
}

macro_rules! format_response {
    ($resp:expr) => {{
        match $resp {
            Ok(x) => {
                let mut out: BytesMut = BytesMut::from("+");
                out.extend_from_slice(&Bytes::from(x));
                out.into()
            }
            Err(e) => format_error_response!(e),
        }
    }};
}

/// Server options
#[derive(Clone, Debug, Serialize, Deserialize, FromArgs)]
struct RemitsConfig {
    #[argh(option, short = 'p')]
    /// what port to start remits on
    pub port: Option<String>,
    // v can change dont care
    #[argh(option, short = 'v')]
    /// verbosity of logs
    pub log_level: Option<String>,
}

impl RemitsConfig {
    fn update_from_flags(&mut self) {
        let flags: RemitsConfig = argh::from_env();
        // This one must be first so debug logs work the rest of the way down
        setup_logger(self.log_level.clone(), flags.log_level);
        if flags.port.is_some() {
            debug!(
                "Replacing config option \"port\":{} with flag \"-p/--port\":{}",
                self.port.as_ref().unwrap(),
                flags.port.as_ref().unwrap()
            );
            self.port = flags.port;
        }
    }
}
/// `RemitsConfig` implements `Default`
impl ::std::default::Default for RemitsConfig {
    fn default() -> Self {
        Self {
            port: Some("4242".into()),
            log_level: Some("info".into()),
        }
    }
}
fn setup_logger(config_level: Option<String>, flag_level: Option<String>) {
    let log_level = &flag_level.unwrap_or(
        config_level
            .as_ref()
            .unwrap_or(&"info".to_owned())
            .to_string(),
    );
    Builder::new()
        .parse_filters(log_level)
        .target(Target::Stdout)
        .format_timestamp_nanos()
        .init();
    debug!("Log level set to {}", log_level);
}

async fn handle_socket(db: Arc<Mutex<db::DB>>, socket: TcpStream) {
    debug!("accepting connection");

    let mut framer = Framed::new(socket, LengthDelimitedCodec::new());

    while let Some(result) = framer.next().await {
        let frame = match result {
            Ok(f) => f,
            Err(e) => {
                error!("could not read from socket: {}", e);
                break;
            }
        };

        debug!("received command: {:?}", &frame);
        let cmd = match parser::parse(&*frame) {
            Ok(cmd) => cmd,
            Err(e) => {
                debug!("responding with: {:?}", e);
                let resp: Bytes = format_error_response!(e);

                let _ = framer.send(resp).await;
                continue;
            }
        };

        let out = db.lock().unwrap().exec(cmd);
        let resp = format_response!(out);

        debug!("responding with: {:?}", resp);
        if let Err(e) = framer.send(resp).await {
            error!("could not respond: {}", e);
        }
    }

    debug!("closing connection");
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut cfg: RemitsConfig = confy::load("remits")?;
    cfg.update_from_flags();

    info!("starting server");

    let db = Arc::new(Mutex::new(db::DB::new()));

    let addr = "0.0.0.0:".to_owned() + &cfg.port.expect("No port defined");
    let mut listener = TcpListener::bind(&addr).await?;
    info!("listening on {}", addr);

    loop {
        match listener.accept().await {
            Ok((socket, _)) => {
                tokio::spawn(handle_socket(db.clone(), socket));
            }
            Err(e) => error!("error accepting listener: {}", e),
        }
    }
}
