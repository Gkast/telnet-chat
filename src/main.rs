use std::sync::Arc;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    net::TcpListener,
    signal::ctrl_c,
    signal::unix::{signal, SignalKind},
    sync::{broadcast, Mutex},
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let pid = std::process::id();
    println!("Server started with PID: {}", pid);

    let listener = TcpListener::bind("localhost:8080").await?;
    let (tx, _rx) = broadcast::channel(10);
    let tx = Arc::new(Mutex::new(tx));

    // Spawn a task to handle the SIGINT signal (Ctrl+C)
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::mpsc::channel(1);
    let tx_clone = Arc::clone(&tx);
    tokio::spawn(async move {
        let _ = ctrl_c().await;
        println!("Received Ctrl+C (PID: {}). Initiating graceful shutdown...", pid);

        // Inform other tasks about shutdown
        let _ = tx_clone.lock().await.send((
            format!("Server (PID: {}) is shutting down due to Ctrl+C", pid),
            std::net::SocketAddr::from(([0, 0, 0, 0], 0)),
        ));

        // Send a message to the main thread to initiate shutdown
        let _ = shutdown_tx.send(()).await;
    });

    // Spawn a task to handle the SIGTERM signal
    let (shutdown_tx_sigterm, mut shutdown_rx_sigterm) = tokio::sync::mpsc::channel(1);
    let tx_clone_sigterm = Arc::clone(&tx);
    tokio::spawn(async move {
        let mut sigterm = signal(SignalKind::terminate()).unwrap();
        sigterm.recv().await;
        println!("Received SIGTERM (PID: {}). Initiating graceful shutdown...", pid);

        // Inform other tasks about shutdown
        let _ = tx_clone_sigterm.lock().await.send((
            format!("Server (PID: {}) is shutting down due to SIGTERM", pid),
            std::net::SocketAddr::from(([0, 0, 0, 0], 0)),
        ));

        // Send a message to the main thread to initiate shutdown
        let _ = shutdown_tx_sigterm.send(()).await;
    });

    let mut connections = vec![];

    loop {
        tokio::select! {
            result = listener.accept() => {
                let (socket, addr) = result?;
                let tx_clone = Arc::clone(&tx);

                // Spawn a task to handle the connection
                let connection = tokio::spawn(handle_connection(socket, addr, tx_clone));
                connections.push(connection);
            }
            _ = shutdown_rx.recv() => {
                // Received shutdown signal, break the loop and exit
                println!("Shutting down gracefully due to Ctrl+C (PID: {})...", pid);

                // Wait for all connection handling tasks to finish
                for connection in connections {
                    let _ = connection.await; // Wait for the task to complete
                }

                break;
            }
            _ = shutdown_rx_sigterm.recv() => {
                // Received shutdown signal, break the loop and exit
                println!("Shutting down gracefully due to SIGTERM (PID: {})...", pid);

                // Wait for all connection handling tasks to finish
                for connection in connections {
                    let _ = connection.await; // Wait for the task to complete
                }

                break;
            }
        }
    }

    Ok(())
}

async fn handle_connection(
    mut socket: tokio::net::TcpStream,
    addr: std::net::SocketAddr,
    tx: Arc<Mutex<tokio::sync::broadcast::Sender<(String, std::net::SocketAddr)>>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (reader, mut _writer) = socket.split();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();

    loop {
        tokio::select! {
            result = reader.read_line(&mut line) => {
                let bytes_read = result?;
                if bytes_read == 0 {
                    break;
                }

                if let Err(e) = tx.lock().await.send((line.clone(), addr)) {
                    eprintln!("Error broadcasting message: {:?}", e);
                }
                line.clear();
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(10)) => {
                // Sleep briefly to avoid busy-waiting
            }
        }
    }

    println!("Connection from {} has been gracefully closed.", addr);
    Ok(())
}
