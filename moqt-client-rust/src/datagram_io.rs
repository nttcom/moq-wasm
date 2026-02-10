use anyhow::{anyhow, Context, Result};
use moqt_core::{
    data_stream_type::DataStreamType,
    messages::data_streams::{datagram, datagram_status, DataStreams},
    variable_integer::read_variable_integer,
};
use std::io::Cursor;
use wtransport::Connection;

pub enum DatagramEvent {
    Object(datagram::Object),
    Status(datagram_status::Object),
}

pub async fn recv_datagram(connection: &Connection) -> Result<DatagramEvent> {
    let datagram = connection
        .receive_datagram()
        .await
        .context("receive datagram")?;
    let mut cursor = Cursor::new(datagram.as_ref());
    let msg_type = read_variable_integer(&mut cursor).context("datagram type")?;
    let stream_type = DataStreamType::try_from(msg_type as u8).context("datagram type")?;
    let payload = &datagram[cursor.position() as usize..];
    let mut read_cur = Cursor::new(payload);
    match stream_type {
        DataStreamType::ObjectDatagram => {
            let obj = datagram::Object::depacketize(&mut read_cur)?;
            Ok(DatagramEvent::Object(obj))
        }
        DataStreamType::ObjectDatagramStatus => {
            let obj = datagram_status::Object::depacketize(&mut read_cur)?;
            Ok(DatagramEvent::Status(obj))
        }
        _ => Err(anyhow!(
            "unsupported datagram stream type: {:?}",
            stream_type
        )),
    }
}
