use axum::body::{to_bytes, Body};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::Response;
use axum::Router;
use morfem_engine::auth::{Authenticator, Open, StaticKeys};
use morfem_engine::{Engine, Request};
use morfem_sqlite::SqliteStore;
use clap::Parser;
use std::sync::Arc;

#[derive(Parser)]
#[command(name = "morfem-server", about = "morfem document database over SQLite")]
struct Args {
    /// SQLite database file.
    #[arg(long, env = "MORFEM_DB", default_value = "morfem.db")]
    db: String,
    #[arg(long, env = "MORFEM_LISTEN", default_value = "127.0.0.1:5984")]
    listen: String,
    /// JSON array of keys: [{"key":"...","access":"admin|write|read","dbs":["a"]}]
    #[arg(long, env = "MORFEM_KEYS")]
    keys: Option<String>,
    /// Disable authentication (everyone is admin). Local development only.
    #[arg(long)]
    no_auth: bool,
}

type App = Arc<Engine<SqliteStore>>;

async fn handle(app: App, req: axum::extract::Request) -> Response {
    let (parts, body) = req.into_parts();
    let Ok(bytes) = to_bytes(body, 2_100_000).await else {
        return plain(StatusCode::PAYLOAD_TOO_LARGE);
    };
    let r = Request {
        method: parts.method.to_string(),
        path: parts.uri.path().to_string(),
        query: parts.uri.query().unwrap_or("").to_string(),
        bearer: parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(str::to_string),
        body: bytes.to_vec(),
    };
    // The engine future is not `Send` (it must also run on single-threaded wasm).
    // SQLite is blocking anyway, so drive it to completion on a blocking thread.
    let rt = tokio::runtime::Handle::current();
    let out = tokio::task::spawn_blocking(move || rt.block_on(app.handle(r))).await;
    match out {
        Ok(resp) => Response::builder()
            .status(resp.status)
            .header(header::CONTENT_TYPE, HeaderValue::from_static(resp.content_type))
            .body(Body::from(resp.body))
            .unwrap(),
        Err(_) => plain(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

fn plain(s: StatusCode) -> Response {
    Response::builder().status(s).body(Body::empty()).unwrap()
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let auth: Box<dyn Authenticator + Send + Sync> = match (&args.keys, args.no_auth) {
        (_, true) => {
            eprintln!("WARNING: authentication disabled");
            Box::new(Open)
        }
        (Some(k), false) => Box::new(StaticKeys::from_json(k).unwrap_or_else(|e| {
            eprintln!("error: {e}");
            std::process::exit(2)
        })),
        (None, false) => {
            eprintln!("error: set MORFEM_KEYS / --keys, or pass --no-auth for local development");
            std::process::exit(2)
        }
    };
    let store = SqliteStore::open(&args.db).unwrap_or_else(|e| {
        eprintln!("error opening {}: {e}", args.db);
        std::process::exit(1)
    });
    let app: App = Arc::new(Engine::new(store, auth, || uuid::Uuid::new_v4().simple().to_string()));
    let router = Router::new().fallback(move |req| handle(app.clone(), req));
    let listener = tokio::net::TcpListener::bind(&args.listen).await.unwrap_or_else(|e| {
        eprintln!("error binding {}: {e}", args.listen);
        std::process::exit(1)
    });
    eprintln!("morfem listening on http://{}", args.listen);
    axum::serve(listener, router).await.unwrap();
}
