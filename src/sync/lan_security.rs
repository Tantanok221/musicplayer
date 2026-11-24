use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use futures_util::{SinkExt, StreamExt};
use snow::{Builder, TransportState};
use tokio::net::TcpStream;
use tokio_tungstenite::{WebSocketStream, tungstenite::Message};

pub type Socket = WebSocketStream<TcpStream>;
pub type PublicKey = [u8; 32];

const MAX_PACKET: usize = 512 * 1024;
const CHUNK: usize = 60_000;
const TAG: usize = 16;

#[derive(Clone, Copy)]
pub enum Trust<'a> {
    Paired(&'a [PublicKey]),
}

#[derive(Debug, thiserror::Error)]
#[error("LAN authentication failed")]
pub struct LinkError;

pub struct Link {
    socket: Socket,
    noise: TransportState,
    packet: Vec<u8>,
}

impl Link {
    pub async fn establish(
        socket: Socket,
        initiator: bool,
        account: &[u8; 32],
        identity: &[u8; 32],
        trust: Trust<'_>,
    ) -> Result<(Self, PublicKey), LinkError> {
        let Trust::Paired(pins) = trust;
        Self::establish_with(socket, initiator, account, identity, |key| async move {
            pins.contains(&key)
        })
        .await
    }

    pub(super) async fn establish_with<F>(
        mut socket: Socket,
        initiator: bool,
        account: &[u8; 32],
        identity: &[u8; 32],
        approve: impl FnOnce(PublicKey) -> F,
    ) -> Result<(Self, PublicKey), LinkError>
    where
        F: Future<Output = bool>,
    {
        let prologue = [b"sonora-lan-v2:".as_slice(), account].concat();
        let builder = Builder::new(
            "Noise_NN_25519_ChaChaPoly_SHA256"
                .parse()
                .map_err(|_| LinkError)?,
        )
        .prologue(&prologue)
        .map_err(|_| LinkError)?;
        let mut handshake = if initiator {
            builder.build_initiator()
        } else {
            builder.build_responder()
        }
        .map_err(|_| LinkError)?;
        let mut send_next = initiator;
        while !handshake.is_handshake_finished() {
            if send_next {
                let mut outgoing = vec![0; 128];
                let len = handshake
                    .write_message(&[], &mut outgoing)
                    .map_err(|_| LinkError)?;
                outgoing.truncate(len);
                send_frame(&mut socket, outgoing).await?;
            } else {
                let incoming = recv_frame(&mut socket, 128).await?;
                if handshake
                    .read_message(&incoming, &mut [])
                    .map_err(|_| LinkError)?
                    != 0
                {
                    return Err(LinkError);
                }
            }
            send_next = !send_next;
        }
        let transcript = handshake.get_handshake_hash().to_vec();
        let mut link = Self {
            socket,
            noise: handshake.into_transport_mode().map_err(|_| LinkError)?,
            packet: Vec::new(),
        };
        let signer = SigningKey::from_bytes(identity);
        let mut proof = signer.verifying_key().to_bytes().to_vec();
        proof.extend_from_slice(&signer.sign(&proof_input(&transcript, initiator)).to_bytes());
        if initiator {
            link.send(&proof).await?;
        }
        let incoming = link.recv().await?;
        let (key, signature) = incoming.split_at_checked(32).ok_or(LinkError)?;
        let key: PublicKey = key.try_into().map_err(|_| LinkError)?;
        VerifyingKey::from_bytes(&key)
            .map_err(|_| LinkError)?
            .verify_strict(
                &proof_input(&transcript, !initiator),
                &Signature::from_slice(signature).map_err(|_| LinkError)?,
            )
            .map_err(|_| LinkError)?;
        if !approve(key).await {
            return Err(LinkError);
        }
        if initiator {
            link.send(&[1]).await?;
        } else {
            link.send(&proof).await?;
            if link.recv().await? != [1] {
                return Err(LinkError);
            }
        }
        Ok((link, key))
    }

    pub async fn send(&mut self, payload: &[u8]) -> Result<(), LinkError> {
        if payload.is_empty() || payload.len() > MAX_PACKET {
            return Err(LinkError);
        }
        let mut chunks = payload.chunks(CHUNK).peekable();
        while let Some(chunk) = chunks.next() {
            let mut plaintext = Vec::with_capacity(1 + chunk.len());
            plaintext.push(u8::from(chunks.peek().is_some()));
            plaintext.extend_from_slice(chunk);
            let mut ciphertext = vec![0; plaintext.len() + TAG];
            let size = self
                .noise
                .write_message(&plaintext, &mut ciphertext)
                .map_err(|_| LinkError)?;
            ciphertext.truncate(size);
            send_frame(&mut self.socket, ciphertext).await?;
        }
        Ok(())
    }

    pub async fn recv(&mut self) -> Result<Vec<u8>, LinkError> {
        loop {
            let frame = recv_frame(&mut self.socket, CHUNK + 1 + TAG).await?;
            let mut plaintext = vec![0; frame.len()];
            let len = self
                .noise
                .read_message(&frame, &mut plaintext)
                .map_err(|_| LinkError)?;
            let (more, chunk) = plaintext[..len].split_first().ok_or(LinkError)?;
            if chunk.is_empty() || self.packet.len() + chunk.len() > MAX_PACKET {
                return Err(LinkError);
            }
            self.packet.extend_from_slice(chunk);
            match more {
                0 => return Ok(std::mem::take(&mut self.packet)),
                1 if chunk.len() == CHUNK => {}
                _ => return Err(LinkError),
            }
        }
    }
}

fn proof_input(transcript: &[u8], initiator: bool) -> Vec<u8> {
    [
        b"sonora-lan-v1-identity:".as_slice(),
        &[u8::from(initiator)],
        transcript,
    ]
    .concat()
}

async fn send_frame(socket: &mut Socket, payload: Vec<u8>) -> Result<(), LinkError> {
    socket
        .send(Message::Binary(payload.into()))
        .await
        .map_err(|_| LinkError)
}

async fn recv_frame(
    socket: &mut Socket,
    max: usize,
) -> Result<tokio_tungstenite::tungstenite::Bytes, LinkError> {
    loop {
        match socket.next().await {
            Some(Ok(Message::Binary(bytes))) if bytes.len() <= max => return Ok(bytes),
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
            _ => return Err(LinkError),
        }
    }
}
