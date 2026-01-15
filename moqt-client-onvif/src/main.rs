mod cli;
mod config;
mod http_client;
mod onvif;
mod rtsp;
mod wsse;
mod soap;
mod ptz;
mod ptz_control;
mod ptz_defs;
mod ptz_input;
mod ptz_parse;
mod ptz_request;
mod ptz_service;

use anyhow::{bail, Result};
use clap::Parser;

#[tokio::main]
async fn main() -> Result<()> {
    let args = cli::Args::parse();
    let target = config::Target::from_args(&args)?;

    if args.ptz {
        ptz::interactive_control(&target, &args).await?;
        return Ok(());
    }

    let (rtsp_result, onvif_result) = tokio::join!(rtsp::probe(&target), onvif::probe(&target));

    let rtsp_ok = report_rtsp(rtsp_result);
    let onvif_ok = report_onvif(onvif_result);

    if !rtsp_ok && !onvif_ok {
        bail!("both RTSP and ONVIF probes failed");
    }

    Ok(())
}

fn report_rtsp(result: Result<rtsp::RtspProbeResult>) -> bool {
    match result {
        Ok(result) => {
            println!("RTSP: {} -> {}", result.endpoint, result.status_line);
            if let Some(code) = result.status_code {
                println!("RTSP status code: {}", code);
            }
            if let Some(auth) = result.www_authenticate {
                println!("RTSP auth challenge: {}", auth);
            }
            true
        }
        Err(err) => {
            eprintln!("RTSP: failed to connect: {err}");
            false
        }
    }
}

fn report_onvif(result: Result<onvif::OnvifProbeResult>) -> bool {
    match result {
        Ok(result) => {
            println!("ONVIF: {} -> HTTP {}", result.endpoint, result.http_status);
            println!("ONVIF body size: {} bytes", result.body_size);
            if result.body.is_empty() {
                println!("ONVIF body is empty");
            } else {
                println!("ONVIF body:\n{}", result.body);
            }
            if result.has_device_info {
                println!("ONVIF device info response detected");
            }
            if result.has_soap_fault {
                println!("ONVIF SOAP fault detected");
            }
            true
        }
        Err(err) => {
            eprintln!("ONVIF: failed to connect: {err}");
            false
        }
    }
}
