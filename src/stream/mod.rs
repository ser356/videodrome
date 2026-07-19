//! Streaming BitTorrent al estilo Stremio (rudimentario): mientras se
//! descarga el fichero, se sirve por HTTP con soporte de Range para que
//! VLC (u otro reproductor) lo pueda reproducir progresivamente.
//!
//! Bajo el capó usa `librqbit` como motor BitTorrent embebido:
//! `handle.stream(file_id)` devuelve un `FileStream` que implementa
//! `AsyncRead + AsyncSeek`. Cada `read()` bloquea hasta que la pieza
//! necesaria está descargada, y registra el rango deseado con el piece
//! picker, que prioriza esas piezas — de facto es "descarga secuencial +
//! primera/última pieza primero" cuando VLC pide byte 0 (cabecera) y luego
//! byte final (para índice `mp4`/`mkv` en algunos casos).
//!
//! ## Caché persistente
//!
//! El fichero se escribe bajo `<cache>/videodrome/streams/<infohash>/` en
//! lugar de un tempdir efímero. Al re-abrir la misma peli, librqbit
//! verifica las piezas ya presentes en disco y arranca casi al instante
//! (sin re-bajar). Si el magnet no expone infohash (raro), se cae a un
//! tempdir tradicional que sí se borra al salir.
//!
//! Cada entrada guarda un fichero `.last_used` que se toca al start y al
//! drop del `StreamHandle`; el módulo `prune` borra las entradas cuyo
//! mtime supere el TTL (configurable en Preferences, default 7 días).
//!
//! ## Resume position
//!
//! El handler HTTP registra el mayor `start` de cada Range con start
//! explícito (los suffix ranges de índice se ignoran) en un `AtomicU64`.
//! Al hacer Drop del `StreamHandle`, se persiste `resume.json` con la
//! fracción `max_seek / file_len`. El caller (GUI) puede leerla con
//! `load_resume(infohash)` y pasar `start_seconds` a `open_in_vlc` para
//! que VLC arranque con `--start-time=<seg>`.
//!
//! ## Estructura del módulo (post-refactor)
//!
//! * `handle` — `StreamHandle`, `start_with_target`, discovery de
//!   ficheros dentro del torrent, `Drop` que persiste resume.
//! * `server` — handlers HTTP del servidor local (`/video`,
//!   `/probe.json`, `/hls/audio`, `/subs/embedded/<idx>`, CORS,
//!   request log).
//! * `hls/` — pipeline HLS: playlist estático, spawn/warmup de
//!   ffmpeg, LRU evict, decisión copy/transcode, argv de audio.
//! * `state` — `AppState`, `HlsState`, `HlsJob`, `HlsMode`, client
//!   capabilities.
//! * `resume` / `cache` / `vlc` — persistencia de posición, gestión
//!   de la caché en disco y del prune, integración VLC (reproductor
//!   externo).

mod cache;
mod handle;
#[cfg(feature = "gui")]
mod hls;
mod resume;
mod server;
mod state;
mod vlc;

#[allow(unused_imports)]
pub use cache::{cache_dir, clear_all, parse_infohash, prune, prune_orphan_tempdirs, total_size};
#[allow(unused_imports)]
pub use handle::{
    compute_moviehash, list_files, select_file, start, start_with_target, StreamHandle,
    StreamStats, TorrentFileInfo,
};
#[allow(unused_imports)]
pub use resume::{load_resume, load_resume_any, save_position, Resume, ResumeEpisode};
#[cfg(feature = "gui")]
pub use state::set_client_capabilities;
#[allow(unused_imports)]
pub use vlc::{open_in_vlc, PlayerHandle};
