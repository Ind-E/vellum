use std::os::linux::net::SocketAddrExt;
use std::os::unix::net::{SocketAddr, UnixDatagram};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::control_socket_name;
use crate::cli::Command;

pub(crate) fn send_command(command: &Command) -> Result<(), String> {
    let socket_addr =
        SocketAddr::from_abstract_name(control_socket_name()).map_err(|error| error.to_string())?;
    let socket = UnixDatagram::unbound().map_err(|error| error.to_string())?;
    socket
        .connect_addr(&socket_addr)
        .map_err(|error| format!("could not connect to the overlay: {error}"))?;

    let message = command.serialize();
    socket
        .send(message.as_bytes())
        .map_err(|error| format!("could not send command: {error}"))?;
    Ok(())
}

pub(crate) fn query(request: Command) -> Result<bool, String> {
    let socket_addr =
        SocketAddr::from_abstract_name(control_socket_name()).map_err(|error| error.to_string())?;
    let reply_name = format!(
        "vellum-query-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_nanos()
    );
    let reply_addr =
        SocketAddr::from_abstract_name(reply_name).map_err(|error| error.to_string())?;
    let socket = UnixDatagram::bind_addr(&reply_addr).map_err(|error| error.to_string())?;
    if let Err(error) = socket.connect_addr(&socket_addr) {
        if error.kind() == std::io::ErrorKind::ConnectionRefused {
            return Ok(false);
        }
        return Err(format!("could not connect to the overlay: {error}"));
    }
    socket
        .set_read_timeout(Some(Duration::from_secs(1)))
        .map_err(|error| format!("could not configure control socket: {error}"))?;
    let request = request.serialize();
    if let Err(error) = socket.send(request.as_bytes()) {
        if error.kind() == std::io::ErrorKind::ConnectionRefused {
            return Ok(false);
        }
        return Err(format!("could not send command: {error}"));
    }

    let mut response = [0; 5];
    let size = match socket.recv(&mut response) {
        Ok(size) => size,
        Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
            return Ok(false);
        }
        Err(error) => return Err(format!("could not receive overlay status: {error}")),
    };
    let active = match &response[..size] {
        b"true" => true,
        b"false" => false,
        _ => return Err("invalid overlay status".into()),
    };
    Ok(active)
}
