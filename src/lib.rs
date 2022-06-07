use std::error::Error;
use std::io::ErrorKind;
use std::sync::Arc;

use structopt::StructOpt;
use x11rb::connection::Connection;
use x11rb::protocol::render::*;
use x11rb::protocol::xproto::ConnectionExt as _;
use x11rb::protocol::xproto::*;
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::protocol::Event;
use x11rb::rust_connection::{DefaultStream, PollMode, RustConnection, Stream as _};

pub mod xauth;

#[derive(Clone, Debug, StructOpt)]
pub struct Config {
    #[structopt(long, default_value = "0")]
    /// Remote X11 screen number
    pub screen: usize,

    #[structopt(long, default_value = "1.0")]
    /// Image scale factor (less than 1 scales down)
    pub scale: f32,
}

impl Default for Config {
    fn default() -> Self {
        Self::from_iter(&[] as &[&str])
    }
}

pub fn run(
    config: &Config,
    rconn: impl Into<Arc<RustConnection>>,
    trconn: impl Into<Arc<RustConnection>>,
) -> Result<(), Box<dyn Error>> {
    let rconn: Arc<RustConnection> = rconn.into();
    let trconn: Arc<RustConnection> = trconn.into();

    let rscreen = &rconn.setup().roots[config.screen];

    let (lconn, lscreen_num) = x11rb::connect(None)?;
    let lconn = Arc::new(lconn);
    let lscreen = &lconn.setup().roots[lscreen_num];
    let win_id = lconn.generate_id()?;

    let pwidth = (config.scale * rscreen.width_in_pixels as f32) as u16;
    let pheight = (config.scale * rscreen.height_in_pixels as f32) as u16;

    // XTEST, RENDER

    //let reply = rconn.list_extensions()?.reply()?;
    //let extensions = reply.names.iter().map(|x| String::from_utf8_lossy(&x.name)).collect::<Vec<_>>();
    //println!("{:?}", extensions);

    let aux = CreateWindowAux::new()
        .background_pixel(lscreen.white_pixel)
        .event_mask(
            EventMask::EXPOSURE
                | EventMask::KEY_PRESS
                | EventMask::KEY_RELEASE
                | EventMask::BUTTON_PRESS
                | EventMask::BUTTON_RELEASE
                | EventMask::POINTER_MOTION,
        );

    lconn.create_window(
        24,
        win_id,
        lscreen.root,
        0,
        0,
        pwidth,
        pheight,
        0,
        WindowClass::INPUT_OUTPUT,
        0,
        &aux,
    )?;

    let name = "Convenient Network Viewer".as_bytes();

    lconn.change_property(
        PropMode::REPLACE,
        win_id,
        AtomEnum::WM_NAME,
        AtomEnum::STRING,
        8,
        name.len() as u32,
        name,
    )?;
    lconn.map_window(win_id)?;
    lconn.flush()?;

    let tconfig = config.clone();
    let tlconn = Arc::clone(&lconn);

    std::thread::spawn(move || {
        stream_picture(tconfig, tlconn, trconn, pwidth, pheight, win_id).expect("error in thread");
    });

    loop {
        let ev = lconn.wait_for_event()?;

        //println!("Event: {:?}", ev);

        match &ev {
            Event::ButtonPress(btn) | Event::ButtonRelease(btn) => {
                rconn
                    .xtest_fake_input(btn.response_type, btn.detail, 0, 0, 0, 0, 0)?
                    .check()?;
            }
            Event::KeyPress(key) | Event::KeyRelease(key) => {
                rconn
                    .xtest_fake_input(key.response_type, key.detail, 0, 0, 0, 0, 0)?
                    .check()?;
            }
            Event::MotionNotify(mot) => {
                let x = (mot.event_x as f32 / config.scale) as i16;
                let y = (mot.event_y as f32 / config.scale) as i16;

                rconn.warp_pointer(0u32, rscreen.root, 0, 0, 0, 0, x, y)?;
            }
            _ => {}
        }
    }
}

pub fn stream_picture(
    config: Config,
    lconn: Arc<RustConnection>,
    rconn: Arc<RustConnection>,
    pwidth: u16,
    pheight: u16,
    win_id: Window,
) -> Result<(), Box<dyn Error>> {
    let rscreen = &rconn.setup().roots[config.screen];

    let gc = lconn.generate_id()?;
    let aux: CreateGCAux = Default::default();
    lconn.create_gc(gc, win_id, &aux)?.check()?;

    let render_opcode = rconn.query_extension(b"RENDER")?.reply()?.major_opcode;

    let formats = rconn.render_query_pict_formats()?.reply()?;

    let fmt = formats
        .formats
        .iter()
        .filter(|f| f.type_ == PictType::DIRECT && f.depth == 24)
        .collect::<Vec<_>>();

    let pic_fmt = fmt[0].id;

    let pixmap = rconn.generate_id()?;

    rconn.create_pixmap(24, pixmap, rscreen.root, pwidth, pheight)?;

    let src_pic = rconn.generate_id()?;
    rconn.render_create_picture(src_pic, rscreen.root, pic_fmt, &Default::default())?;
    let dst_pic = rconn.generate_id()?;
    rconn.render_create_picture(dst_pic, pixmap, pic_fmt, &Default::default())?;

    let xform = Transform {
        matrix11: (1.0 / config.scale * 65536.0) as i32,
        matrix12: 0,
        matrix13: 0,
        matrix21: 0,
        matrix22: (1.0 / config.scale * 65536.0) as i32,
        matrix23: 0,
        matrix31: 0,
        matrix32: 0,
        matrix33: 65536,
    };

    rconn.render_set_picture_transform(src_pic, xform)?;

    rconn.flush()?;

    let stream = rconn.stream();

    let mut buf = vec![0; (pwidth as usize) * (pheight as usize) * 4 + 256];
    let mut pos = 0;
    let mut fds = vec![];

    fn write_request(
        stream: &DefaultStream,
        bytes: Vec<std::borrow::Cow<'static, [u8]>>,
    ) -> std::io::Result<()> {
        let mut fds = vec![];

        for b in bytes {
            let mut pos = 0;

            while pos < b.len() {
                pos += match stream.write(&b[pos..], &mut fds) {
                    Ok(count) => count,
                    Err(e) if e.kind() == ErrorKind::WouldBlock => 0,
                    Err(e) => Err(e)?,
                }
            }
        }

        Ok(())
    }

    loop {
        std::thread::sleep(std::time::Duration::from_millis(16));

        let render_req = CompositeRequest {
            op: PictOp::SRC,
            src: src_pic,
            mask: 0,
            dst: dst_pic,
            src_x: 0,
            src_y: 0,
            mask_x: 0,
            mask_y: 0,
            dst_x: 0,
            dst_y: 0,
            width: pwidth,
            height: pheight,
        };

        write_request(stream, render_req.serialize(render_opcode).0)?;

        let get_image_req = GetImageRequest {
            format: ImageFormat::Z_PIXMAP,
            drawable: pixmap,
            x: 0,
            y: 0,
            width: pwidth,
            height: pheight,
            plane_mask: 0xe0f0e0,
            //plane_mask: 0xffffff,
        };

        write_request(stream, get_image_req.serialize().0)?;

        'read: loop {
            stream.poll(PollMode::Readable)?;

            pos += match stream.read(&mut buf[pos..], &mut fds) {
                Ok(count) => count,
                Err(e) if e.kind() == ErrorKind::WouldBlock => 0,
                Err(e) => Err(e)?,
            };

            while pos > 8 {
                match buf[0] {
                    1 => {
                        let len =
                            u32::from_le_bytes(buf[4..8].try_into().unwrap()) as usize * 4 + 32;

                        if pos >= len {
                            let response = &buf[0..len];

                            lconn.put_image(
                                ImageFormat::Z_PIXMAP,
                                win_id,
                                gc,
                                pwidth,
                                pheight,
                                0,
                                0,
                                0,
                                24,
                                //&img.data,
                                //&img.as_ref()[32..],
                                &response[32..],
                            )?;

                            buf.copy_within(len.., 0);
                            pos -= len;
                            break 'read;
                        } else {
                            break;
                        }
                    }
                    x => {
                        panic!("don't know how to handle {}", x);
                    }
                }
            }
        }
    }
}
