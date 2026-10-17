//! One running instance per data directory. Starting the app again asks the instance
//! that is running to show its window, instead of a second one watching the same
//! folders and writing the same files.

use std::fs::{File, TryLockError};
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;

/// Held for as long as this process is the instance for its data directory.
pub struct InstanceLock {
    _file: File,
    endpoint: ipc::Endpoint,
}

impl InstanceLock {
    /// Takes the lock on `data_dir`, or returns `None` while another process holds it.
    pub fn acquire(data_dir: &Path) -> io::Result<Option<Self>> {
        let file = File::options().create(true).truncate(false).write(true).open(data_dir.join("instance.lock"))?;
        match file.try_lock() {
            Ok(()) => Ok(Some(Self { _file: file, endpoint: ipc::endpoint(data_dir) })),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(error)) => Err(error),
        }
    }

    /// Starts taking requests from later starts of the app to show the window.
    pub fn listen_for_activation(&self, runtime: &tokio::runtime::Handle) -> io::Result<ActivationRequests> {
        let requests = ActivationRequests::default();
        ipc::serve(&self.endpoint, runtime, requests.clone())?;
        Ok(requests)
    }
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        ipc::remove(&self.endpoint);
    }
}

/// Asks the instance that holds `data_dir` to show its window.
pub fn request_activation(data_dir: &Path) -> io::Result<()> {
    ipc::connect(&ipc::endpoint(data_dir))
}

/// Names the data directory in places shared beyond it, which also keeps names short.
fn instance_id(data_dir: &Path) -> String {
    let data_dir = std::fs::canonicalize(data_dir).unwrap_or_else(|_| data_dir.to_path_buf());
    let digest = ring::digest::digest(&ring::digest::SHA256, data_dir.as_os_str().as_encoded_bytes());
    digest.as_ref()[..8].iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Requests from later starts of the app to show the window.
#[derive(Clone, Default)]
pub struct ActivationRequests(Arc<RequestState>);

#[derive(Default)]
struct RequestState {
    pending: AtomicBool,
    wake: Mutex<Option<Box<dyn Fn() + Send>>>,
}

impl ActivationRequests {
    /// Calls `wake` whenever a request arrives.
    pub fn set_waker(&self, wake: impl Fn() + Send + 'static) {
        *self.0.wake.lock() = Some(Box::new(wake));
    }

    /// Whether a request arrived since the last call.
    pub fn take(&self) -> bool {
        self.0.pending.swap(false, Ordering::AcqRel)
    }

    fn signal(&self) {
        self.0.pending.store(true, Ordering::Release);
        if let Some(wake) = &*self.0.wake.lock() {
            wake();
        }
    }
}

#[cfg(unix)]
mod ipc {
    use std::io;
    use std::path::{Path, PathBuf};

    use super::{instance_id, ActivationRequests};

    pub type Endpoint = PathBuf;

    /// In the user's runtime directory: socket paths are limited to about a hundred bytes,
    /// which a data directory given with `--home` can exceed on its own.
    pub fn endpoint(data_dir: &Path) -> Endpoint {
        let dirs = directories::BaseDirs::new();
        let runtime_dir = dirs.as_ref().and_then(|dirs| dirs.runtime_dir()).map_or_else(std::env::temp_dir, Path::to_path_buf);
        runtime_dir.join(format!("llm-subs-{}.sock", instance_id(data_dir)))
    }

    pub fn serve(socket: &Endpoint, runtime: &tokio::runtime::Handle, requests: ActivationRequests) -> io::Result<()> {
        // Only the lock holder gets here, so a socket that is still there was left behind
        // by an instance that is gone.
        remove(socket);
        let _context = runtime.enter();
        let listener = tokio::net::UnixListener::bind(socket)?;
        runtime.spawn(async move {
            loop {
                if let Err(error) = listener.accept().await {
                    tracing::warn!(%error, "stopped listening for other starts of the app");
                    return;
                }
                requests.signal();
            }
        });
        Ok(())
    }

    pub fn connect(socket: &Endpoint) -> io::Result<()> {
        std::os::unix::net::UnixStream::connect(socket).map(drop)
    }

    pub fn remove(socket: &Endpoint) {
        if let Err(error) = std::fs::remove_file(socket)
            && error.kind() != io::ErrorKind::NotFound
        {
            tracing::warn!(%error, path = %socket.display(), "could not remove the instance socket");
        }
    }
}

#[cfg(windows)]
mod ipc {
    use std::io;
    use std::path::Path;

    use tokio::net::windows::named_pipe::ServerOptions;

    use super::{instance_id, ActivationRequests};

    pub type Endpoint = String;

    pub fn endpoint(data_dir: &Path) -> Endpoint {
        format!(r"\\.\pipe\llm-subs-{}", instance_id(data_dir))
    }

    pub fn serve(name: &Endpoint, runtime: &tokio::runtime::Handle, requests: ActivationRequests) -> io::Result<()> {
        let _context = runtime.enter();
        let mut server = ServerOptions::new().first_pipe_instance(true).create(name)?;
        let name = name.clone();
        runtime.spawn(async move {
            loop {
                if let Err(error) = server.connect().await {
                    tracing::warn!(%error, "stopped listening for other starts of the app");
                    return;
                }
                // Each pipe instance takes one caller; the next caller needs a new one.
                server = match ServerOptions::new().create(&name) {
                    Ok(next) => next,
                    Err(error) => {
                        tracing::warn!(%error, "stopped listening for other starts of the app");
                        return;
                    }
                };
                requests.signal();
            }
        });
        Ok(())
    }

    pub fn connect(name: &Endpoint) -> io::Result<()> {
        std::fs::OpenOptions::new().write(true).open(name).map(drop)
    }

    pub fn remove(_name: &Endpoint) {}
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    #[test]
    fn only_one_process_holds_a_data_directory() {
        let dir = tempfile::tempdir().unwrap();
        let first = InstanceLock::acquire(dir.path()).unwrap();
        assert!(first.is_some());
        assert!(InstanceLock::acquire(dir.path()).unwrap().is_none());
        drop(first);
        assert!(InstanceLock::acquire(dir.path()).unwrap().is_some());
    }

    #[test]
    fn a_later_start_reaches_the_running_instance() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();
        assert!(request_activation(dir.path()).is_err(), "nothing is listening yet");

        let lock = InstanceLock::acquire(dir.path()).unwrap().unwrap();
        let requests = lock.listen_for_activation(runtime.handle()).unwrap();
        assert!(!requests.take());
        request_activation(dir.path()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !requests.take() {
            assert!(Instant::now() < deadline, "the request never arrived");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
