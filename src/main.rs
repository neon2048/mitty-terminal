use anyhow::{bail, Result};
use core::str;
use embedded_svc::{
    http::{client::Client, Method},
    io::Read,
};
use esp_idf_svc::{
    eventloop::EspSystemEventLoop,
    hal::{peripheral, prelude::Peripherals},
    http::client::EspHttpConnection,
    wifi::{AuthMethod, BlockingWifi, ClientConfiguration, Configuration, EspWifi},
};
use log::info;

#[toml_cfg::toml_config]
pub struct Config {
    #[default("")]
    wifi_ssid: &'static str,
    #[default("")]
    wifi_psk: &'static str,
}

fn main() -> Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    let peripherals = Peripherals::take().unwrap();
    let sysloop = EspSystemEventLoop::take()?;

    let app_config = CONFIG;

    // Connect to the Wi-Fi network
    let _wifi = wifi(
        app_config.wifi_ssid,
        app_config.wifi_psk,
        peripherals.modem,
        sysloop,
    )?;

    get("https://mitty-terminal.uwu.ai/")?;

    Ok(())
}

pub fn wifi(
    ssid: &str,
    pass: &str,
    modem: impl peripheral::Peripheral<P = esp_idf_svc::hal::modem::Modem> + 'static,
    sysloop: EspSystemEventLoop,
) -> Result<Box<EspWifi<'static>>> {
    let mut auth_method = AuthMethod::WPA2Personal;
    if ssid.is_empty() {
        bail!("Missing WiFi name")
    }
    if pass.is_empty() {
        auth_method = AuthMethod::None;
        info!("Wifi password is empty");
    }
    let mut esp_wifi = EspWifi::new(modem, sysloop.clone(), None)?;

    let mut wifi = BlockingWifi::wrap(&mut esp_wifi, sysloop)?;

    wifi.set_configuration(&Configuration::Client(ClientConfiguration::default()))?;

    info!("Starting wifi...");

    wifi.start()?;

    info!("Scanning...");

    let ap_infos = wifi.scan()?;

    let ours = ap_infos.into_iter().find(|a| a.ssid == ssid);

    let channel = if let Some(ours) = ours {
        info!(
            "Found configured access point {} on channel {}",
            ssid, ours.channel
        );
        Some(ours.channel)
    } else {
        info!(
            "Configured access point {} not found during scanning, will go with unknown channel",
            ssid
        );
        None
    };

    wifi.set_configuration(&Configuration::Client(ClientConfiguration {
        ssid: ssid
            .try_into()
            .expect("Could not parse the given SSID into WiFi config"),
        password: pass
            .try_into()
            .expect("Could not parse the given password into WiFi config"),
        channel,
        auth_method,
        ..Default::default()
    }))?;

    info!("Connecting wifi...");

    wifi.connect()?;

    info!("Waiting for DHCP lease...");

    wifi.wait_netif_up()?;

    let ip_info = wifi.wifi().sta_netif().get_ip_info()?;

    info!("Wifi DHCP info: {:?}", ip_info);

    Ok(Box::new(esp_wifi))
}

fn get(url: impl AsRef<str>) -> Result<()> {
    // 1. Create a new EspHttpClient. (Check documentation)
    // ANCHOR: connection
    let connection = EspHttpConnection::new(&esp_idf_svc::http::client::Configuration {
        use_global_ca_store: true,
        crt_bundle_attach: Some(esp_idf_svc::sys::esp_crt_bundle_attach),
        ..Default::default()
    })?;
    // ANCHOR_END: connection
    let mut client = Client::wrap(connection);

    // 2. Open a GET request to `url`
    let headers = [("accept", "text/plain")];
    let request = client.request(Method::Get, url.as_ref(), &headers)?;

    // 3. Submit write request and check the status code of the response.
    // Successful http status codes are in the 200..=299 range.
    let response = request.submit()?;
    let status = response.status();

    println!("Response code: {}\n", status);

    match status {
        200..=299 => {
            // 4. if the status is OK, read response data chunk by chunk into a buffer and print it until done
            //
            // NB. see http_client.rs for an explanation of the offset mechanism for handling chunks that are
            // split in the middle of valid UTF-8 sequences. This case is encountered a lot with the given
            // example URL.
            let mut buf = [0_u8; 11];
            let mut offset = 0;
            let mut total = 0;
            let mut reader = response;
            let mut state = ChunkMatchState::new();
            loop {
                if let Ok(size) = Read::read(&mut reader, &mut buf[offset..]) {
                    if size == 0 {
                        break;
                    }
                    total += size;
                    // 5. try converting the bytes into a Rust (UTF-8) string and print it
                    let size_plus_offset = size + offset;
                    match str::from_utf8(&buf[..size_plus_offset]) {
                        Ok(text) => {
                            handle_chunk(text, &mut state);
                            offset = 0;
                        }
                        Err(error) => {
                            let valid_up_to = error.valid_up_to();
                            unsafe {
                                // print!("{}", str::from_utf8_unchecked(&buf[..valid_up_to]));
                                handle_chunk(
                                    str::from_utf8_unchecked(&buf[..valid_up_to]),
                                    &mut state,
                                );
                            }
                            buf.copy_within(valid_up_to.., 0);
                            offset = size_plus_offset - valid_up_to;
                        }
                    }
                }
            }
            println!("Total: {} bytes", total);
        }
        _ => bail!("Unexpected response code: {}", status),
    }

    Ok(())
}

#[derive(PartialEq, Eq)]
enum MatchStep {
    FindPreamble,
    FindHeaderStart,
    FindHeaderEnd,
    FindBodyStart,
    FindBodyEnd,
    ResultAvailable,
}

struct ChunkMatchState {
    needle_offset: usize,
    current_step: MatchStep,
    header: String,
    body: String,
}

impl ChunkMatchState {
    fn new() -> Self {
        Self {
            needle_offset: 0,
            current_step: MatchStep::FindPreamble,
            header: String::with_capacity("22/11 4:00".len()),
            body: String::with_capacity(50),
        }
    }
}

fn handle_chunk(chunk: &str, state: &mut ChunkMatchState) {
    let mut cur_chunk = chunk;
    while let Some(rest) = handle_chunk_element(cur_chunk, state) {
        cur_chunk = rest;

        if state.current_step == MatchStep::ResultAvailable {
            if state.header.contains("#update-board-archive") {
                return;
            }

            unescape(&mut state.header);
            unescape(&mut state.body);
            println!("Header: {}", state.header);
            println!("Body: {}", state.body);
            state.header.clear();
            state.body.clear();
        }
    }
}

struct ReplacementItem<'a> {
    from: &'a [u8],
    to: u8,
}

const REPLACMENTS: &[ReplacementItem] = &[
    ReplacementItem {
        from: b"<br>",
        to: b'\n',
    },
    ReplacementItem {
        from: b"&nbsp;",
        to: b' ',
    },
    ReplacementItem {
        from: b"&lt;",
        to: b'<',
    },
    ReplacementItem {
        from: b"&gt;",
        to: b'>',
    },
    ReplacementItem {
        from: b"&amp;",
        to: b'&',
    },
    ReplacementItem {
        from: b"&quot;",
        to: b'"',
    },
    ReplacementItem {
        from: b"&apos;",
        to: b'\'',
    },
];
const ENTITY_START: &[u8] = b"&#";
const ENTITY_END: u8 = b';';

// scuffed in-place HTML entity decoding \🧿/
fn unescape(text: &mut String) {
    let mut buffer = std::mem::take(text).into_bytes();

    let mut newsize = 0;

    let mut wpos: usize = 0;

    let mut repl_idx: [usize; REPLACMENTS.len()] = [0; REPLACMENTS.len()];

    let mut pidx = 0;
    let mut summode: bool = false;
    let mut sum: u8 = 0;
    let mut sum_cnt: usize = 0;

    for i in 0..buffer.len() {
        let mut b = buffer[i];

        for ridx in 0..REPLACMENTS.len() {
            if b == REPLACMENTS[ridx].from[repl_idx[ridx]] {
                repl_idx[ridx] += 1;
            } else {
                repl_idx[ridx] = 0;
            }

            if repl_idx[ridx] == REPLACMENTS[ridx].from.len() {
                repl_idx.iter_mut().map(|x| *x = 0).count();
                wpos -= REPLACMENTS[ridx].from.len() - 1;
                newsize -= REPLACMENTS[ridx].from.len() - 1;
                b = REPLACMENTS[ridx].to;
            }
        }

        if b == ENTITY_END && summode {
            summode = false;

            b = sum;
            wpos -= sum_cnt + ENTITY_START.len();
            newsize -= sum_cnt + ENTITY_START.len();

            sum = 0;
            pidx = 0;
            sum_cnt = 0;
        }

        if summode {
            sum = sum * 10;
            sum += b - b'0';
            sum_cnt += 1;
        }

        if b == ENTITY_START[pidx] {
            pidx += 1;
        } else {
            pidx = 0;
        }

        if pidx == ENTITY_START.len() {
            pidx = 0;
            summode = true;
        }

        buffer[wpos] = b;

        // Mitty likes leading spaces... I don't
        if !(wpos == 0 && b == b' ') {
            wpos += 1;
            newsize += 1;
        }
    }

    buffer.resize(newsize, b'\0');
    *text = String::from_utf8(buffer).expect("unescape failed, invalid UTF-8")
}

fn handle_chunk_element<'a>(chunk: &'a str, state: &mut ChunkMatchState) -> Option<&'a str> {
    match state.current_step {
        MatchStep::FindPreamble => {
            let needle = "Scroll to the right to read!";
            let offset = find_needle_chunked(needle, chunk, state);
            let (found, rest) = handle_tag_start(offset, chunk);
            if found {
                state.current_step = MatchStep::FindHeaderStart;
            }
            return rest;
        }

        MatchStep::FindHeaderStart => {
            let needle = "<strong>";
            let offset = find_needle_chunked(needle, chunk, state);
            let (found, rest) = handle_tag_start(offset, chunk);
            if found {
                state.current_step = MatchStep::FindHeaderEnd;
            }
            return rest;
        }

        MatchStep::FindHeaderEnd => {
            let needle = "</strong>";
            let offset = find_needle_chunked(needle, chunk, state);
            let (found, rest) = handle_tag_end(offset, needle, chunk, &mut state.header);
            if found {
                state.current_step = MatchStep::FindBodyStart;
            }

            return rest;
        }

        MatchStep::FindBodyStart => {
            let needle = "</span>";
            let offset = find_needle_chunked(needle, chunk, state);
            let (found, rest) = handle_tag_start(offset, chunk);
            if found {
                state.current_step = MatchStep::FindBodyEnd;
            }
            return rest;
        }

        MatchStep::FindBodyEnd => {
            let needle = "</td>";
            let offset = find_needle_chunked(needle, chunk, state);
            let (found, rest) = handle_tag_end(offset, needle, chunk, &mut state.body);
            if found {
                state.current_step = MatchStep::ResultAvailable;
            }

            return rest;
        }

        MatchStep::ResultAvailable => {
            state.current_step = MatchStep::FindHeaderStart;
            return Some(chunk);
        }
    }
}

fn handle_tag_end<'a>(
    offset: Option<usize>,
    preamble: &str,
    chunk: &'a str,
    buffer: &mut String,
) -> (bool, Option<&'a str>) {
    match offset {
        Some(offset) => {
            let print_rest = chunk.get(0..offset - preamble.len());
            let rest = chunk.get(offset..);
            if let Some(print_rest) = print_rest {
                buffer.push_str(print_rest);
            } else {
                // special case
                // if the needle we are looking for was partially
                // at the end of the previous chunk, a part of it
                // has been written to the result buffer
                // in this case, we remove that part
                let cut = preamble.len() - offset;
                buffer.truncate(buffer.len() - cut);
            }
            return (true, rest);
        }

        None => {
            buffer.push_str(chunk);
            return (false, None);
        }
    }
}

fn handle_tag_start<'a>(offset: Option<usize>, chunk: &'a str) -> (bool, Option<&'a str>) {
    match offset {
        Some(offset) => {
            let rest = chunk.get(offset..);
            return (true, rest);
        }
        None => (false, None),
    }
}

fn find_needle_chunked(needle: &str, chunk: &str, state: &mut ChunkMatchState) -> Option<usize> {
    let needle = needle.as_bytes();

    for (i, b) in chunk.bytes().enumerate() {
        if b == needle[state.needle_offset] {
            state.needle_offset += 1;
        } else {
            state.needle_offset = 0;
        }

        if state.needle_offset == needle.len() {
            state.needle_offset = 0;
            return Some(i + 1);
        }
    }

    return None;
}
