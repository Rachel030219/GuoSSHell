use std::collections::VecDeque;

use rshell_core::{ExitStatus, SessionFailure};
use russh::{Channel, ChannelMsg, client};

use crate::{TransportError, TransportEvent, TransportRequest};

pub(super) async fn configure_channel(
    channel: &mut Channel<client::Msg>,
    request: &TransportRequest,
    remote_command: Option<&str>,
    pending: &mut VecDeque<TransportEvent>,
) -> Result<(), TransportError> {
    let size = request.initial_size();
    channel
        .request_pty(
            true,
            request.terminal_type(),
            u32::from(size.cols),
            u32::from(size.rows),
            size.pixel_width,
            size.pixel_height,
            &[],
        )
        .await
        .map_err(|_| channel_error())?;
    await_success(channel, pending).await?;
    match remote_command {
        Some(command) => channel.exec(true, command.as_bytes()).await,
        None => channel.request_shell(true).await,
    }
    .map_err(|_| channel_error())?;
    await_success(channel, pending).await
}

async fn await_success(
    channel: &mut Channel<client::Msg>,
    pending: &mut VecDeque<TransportEvent>,
) -> Result<(), TransportError> {
    loop {
        match channel.wait().await {
            Some(ChannelMsg::Success) => return Ok(()),
            Some(ChannelMsg::Failure | ChannelMsg::OpenFailure(_) | ChannelMsg::Close) | None => {
                return Err(channel_error());
            }
            message => {
                if let Some(event) = channel_event(message) {
                    pending.push_back(event);
                }
            }
        }
    }
}

pub(super) fn channel_event(message: Option<ChannelMsg>) -> Option<TransportEvent> {
    match message {
        Some(ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. }) => {
            Some(TransportEvent::Output(data.to_vec()))
        }
        Some(ChannelMsg::ExitStatus { exit_status }) => {
            let code = i32::try_from(exit_status).ok();
            Some(TransportEvent::Exit(ExitStatus {
                code,
                success: code == Some(0),
            }))
        }
        Some(ChannelMsg::ExitSignal { .. }) => Some(TransportEvent::Exit(ExitStatus {
            code: None,
            success: false,
        })),
        Some(ChannelMsg::Eof | ChannelMsg::Close) => Some(TransportEvent::Eof),
        None => Some(TransportEvent::Failure(TransportError::new(
            SessionFailure::Network,
        ))),
        Some(ChannelMsg::Failure | ChannelMsg::OpenFailure(_)) => Some(TransportEvent::Failure(
            TransportError::new(SessionFailure::SshChannel),
        )),
        Some(
            ChannelMsg::Open { .. }
            | ChannelMsg::RequestPty { .. }
            | ChannelMsg::RequestShell { .. }
            | ChannelMsg::Exec { .. }
            | ChannelMsg::Signal { .. }
            | ChannelMsg::RequestSubsystem { .. }
            | ChannelMsg::RequestX11 { .. }
            | ChannelMsg::SetEnv { .. }
            | ChannelMsg::WindowChange { .. }
            | ChannelMsg::AgentForward { .. }
            | ChannelMsg::XonXoff { .. }
            | ChannelMsg::WindowAdjusted { .. }
            | ChannelMsg::Success,
        ) => None,
        Some(_) => None,
    }
}

fn channel_error() -> TransportError {
    TransportError::new(SessionFailure::SshChannel)
}
