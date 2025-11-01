use std::env;

use error::PrinterBotError;
use image::{ImageBuffer, Luma, Rgba};
use log::*;
use teloxide_core::net::Download;
use teloxide_core::types::{ChatId, FileId};
use teloxide_core::{
    payloads::GetUpdatesSetters,
    requests::{Requester, RequesterExt},
};

use crate::driver::{PrinterCommand, PrinterCommandMode, PrinterExpandedMode, PrinterMode};

mod driver;
mod error;

#[derive(Debug)]
struct Settings {
    dpi_600: bool,
    auto_cut: bool,
    dithering: bool,
}

#[tokio::main]
async fn main() -> Result<(), PrinterBotError> {
    dotenvy::dotenv().ok();
    env_logger::init();

    let token = env::var("BOT_TOKEN").expect("BOT_TOKEN is not set");
    let owner_id: ChatId = ChatId(
        env::var("OWNER_ID")
            .expect("OWNER_ID is not set")
            .parse()
            .expect("invalid OWNER_ID"),
    );

    let bot = teloxide_core::Bot::new(token).parse_mode(teloxide_core::types::ParseMode::Html);

    bot.send_message(owner_id, "sto partendo").await?;

    info!("Started polling");

    let mut offset: u32 = 0;

    let settings = Settings {
        dpi_600: false,
        auto_cut: true,
        dithering: true,
    };

    info!("Settings: {:?}", settings);
    bot.send_message(owner_id, format!("Settings: {:?}", settings))
        .await?;

    loop {
        let updates = bot.get_updates().offset(offset as i32).await;

        match updates {
            Ok(updates) => {
                for update in updates {
                    offset = update.id.0 + 1;

                    if let teloxide_core::types::UpdateKind::Message(message) = update.kind {
                        if message.chat.id != owner_id {
                            continue;
                        }

                        if let Some((file_id, file_ext)) =
                            extract_photo_from_message(&bot, &message).await?
                        {
                            let file_path = download_file(&bot, &file_id, &file_ext).await?;

                            let lines = render_image(&file_path, &settings)?;

                            if let Err(err) = print_lines(lines, &settings) {
                                error!("print failed, {:?}", err);
                            }
                        }
                    }
                }
            }
            Err(err) => {
                error!("{:?}", err);
                bot.send_message(owner_id, format!("{:#?}", err)).await.ok();
            }
        }
    }
}

async fn extract_photo_from_message(
    bot: &teloxide_core::adaptors::DefaultParseMode<teloxide_core::Bot>,
    message: &teloxide_core::types::Message,
) -> Result<Option<(String, String)>, PrinterBotError> {
    if let Some(photo) = message.photo() {
        let biggest = photo.iter().max_by_key(|x| x.width);

        if let Some(biggest) = biggest {
            return Ok(Some((biggest.file.id.to_string(), "jpg".to_string())));
        }
    }

    if let Some(sticker) = message.sticker() {
        if sticker.is_static() {
            return Ok(Some((sticker.file.id.to_string(), "webp".to_string())));
        } else {
            bot.send_message(message.chat.id, "Can't print animated stickers")
                .await?;
        }
    }

    if let Some(document) = message.document() {
        if let Some(mime_type) = &document.mime_type {
            let extension = match mime_type.as_ref() {
                "image/jpeg" => "jpg",
                "image/png" => "png",
                "image/gif" => "gif",
                "image/webp" => "webp",
                "image/tiff" => "tiff",
                "image/bmp" => "bmp",
                _ => {
                    bot.send_message(message.chat.id, "Can't print documents that are not images")
                        .await?;
                    return Ok(None);
                }
            };

            return Ok(Some((document.file.id.to_string(), extension.to_string())));
        }

        return Ok(None);
    }

    Ok(None)
}

async fn download_file(
    bot: &teloxide_core::adaptors::DefaultParseMode<teloxide_core::Bot>,
    file_id: &str,
    file_ext: &str,
) -> Result<String, PrinterBotError> {
    let file = bot.get_file(FileId::from(file_id.to_string())).await?;
    let file_path = format!("/tmp/toprint.{file_ext}");
    let mut dst = tokio::fs::File::create(&file_path).await?;
    bot.download_file(&file.path, &mut dst).await?;
    Ok(file_path)
}

fn apply_dithering(
    mut input_img: ImageBuffer<Luma<u8>, Vec<u8>>,
) -> Result<ImageBuffer<Rgba<u8>, Vec<u8>>, PrinterBotError> {
    // match the brightness of the previous implementation
    let gamma_correction = 5.14;

    input_img
        .pixels_mut()
        .for_each(|x| x.0 = [(255.0 * (x.0[0] as f32 / 255.0).powf(1.0 / gamma_correction)) as u8]);

    use exoquant::*;

    let palette = vec![Color::new(0, 0, 0, 255), Color::new(255, 255, 255, 255)];

    let ditherer = ditherer::FloydSteinberg::vanilla();
    let colorspace = SimpleColorSpace::default();
    let remapper = Remapper::new(&palette, &colorspace, &ditherer);

    let image = input_img
        .pixels()
        .map(|x| Color::new(x.0[0], x.0[0], x.0[0], 255))
        .collect::<Vec<Color>>();

    let indexed_data = remapper.remap(&image, input_img.width() as usize);

    let img = image::ImageBuffer::from_fn(input_img.width(), input_img.height(), |x, y| {
        let i = y * input_img.width() + x;
        let i = indexed_data[i as usize];
        image::Rgba([i * 255, i * 255, i * 255, 255])
    });

    Ok(img)
}

fn apply_threshold(
    mut img: ImageBuffer<Luma<u8>, Vec<u8>>,
) -> Result<ImageBuffer<Rgba<u8>, Vec<u8>>, PrinterBotError> {
    img.pixels_mut().for_each(|x| {
        if x.0[0] > 128 {
            x.0[0] = 255;
        } else {
            x.0[0] = 0;
        }
    });

    let img = image::ImageBuffer::from_fn(img.width(), img.height(), |x, y| {
        let i = y * img.width() + x;
        let i = img.get_pixel(x, y).0[0];
        image::Rgba([i, i, i, 255])
    });

    Ok(img)
}

fn img_to_lines(img: ImageBuffer<Rgba<u8>, Vec<u8>>) -> Result<Vec<[u8; 90]>, PrinterBotError> {
    // convert to vec of line bits
    /*
        let mut lines = Vec::new();

        for y in 0..img.height() {
            let mut line = [0u8; 90];

            for x in 0..img.width() {
                let i = y * img.width() + x;
                let i = indexed_data[i as usize];

                let byte = x / 8;
                let bit = x % 8;

                if i == 0 {
                    line[89 - byte as usize] |= 1 << bit;
                }
            }

            lines.push(line);
        }
    */

    let mut lines = Vec::new();

    for y in 0..img.height() {
        let mut line = [0u8; 90];

        for x in 0..img.width() {
            let i = img.get_pixel(x, y).0[0];

            let byte = x / 8;
            let bit = x % 8;

            // if the pixel is black, set the bit so it's printed (in black)
            if i == 0 {
                line[89 - byte as usize] |= 1 << bit;
            }
        }

        lines.push(line);
    }

    Ok(lines)
}

fn render_image(file_path: &str, settings: &Settings) -> Result<Vec<[u8; 90]>, PrinterBotError> {
    use image::ImageReader;

    let img = ImageReader::open(file_path)?.decode()?;

    // Limit stickers ratio (so people don't print incredibly long stickers)

    let ratio = img.height() as f32 / img.width() as f32;

    if ratio > 3.5 {
        println!("Ratio is too high: {}", ratio);
        return Err(PrinterBotError::InvalidImage);
    }

    // remove transparency
    let img = img.into_rgba8();

    let background_color = image::Rgba([255, 255, 255, 255]);
    let mut background_image =
        image::ImageBuffer::from_pixel(img.width(), img.height(), background_color);
    image::imageops::overlay(&mut background_image, &img, 0, 0);

    // convert to grayscale

    let img = image::imageops::grayscale(&background_image);

    // resize

    let new_width = 720; //630 per la carta piccola

    let new_height = new_width * img.height() / img.width() * if settings.dpi_600 { 2 } else { 1 };

    let mut img = image::imageops::resize(
        &img,
        new_width,
        new_height,
        image::imageops::FilterType::Lanczos3,
    );

    let dithered_img = if settings.dithering {
        apply_dithering(img)?
    } else {
        apply_threshold(img)?
    };

    dithered_img.save("/tmp/out_processed.png")?;

    let lines = img_to_lines(dithered_img)?;
    Ok(lines)
}

fn print_lines(lines: Vec<[u8; 90]>, settings: &Settings) -> Result<(), PrinterBotError> {
    let mut printer = driver::PrinterCommander::main("/dev/usb/lp0")?;

    printer.send_command(PrinterCommand::Reset)?;
    printer.send_command(PrinterCommand::Initialize)?;

    // information
    printer.send_command(PrinterCommand::StatusInfoRequest)?;

    let status = printer.read_status()?;
    trace!("{:#?}", status);

    printer.send_command(PrinterCommand::SetCommandMode(PrinterCommandMode::Raster))?;

    printer.send_command(PrinterCommand::SetPrintInformation(
        status,
        lines.len() as i32,
    ))?;

    printer.send_command(PrinterCommand::SetExpandedMode(PrinterExpandedMode {
        cut_at_end: settings.auto_cut,
        high_resolution_printing: settings.dpi_600,
    }))?;

    printer.send_command(PrinterCommand::SetMode(PrinterMode {
        auto_cut: settings.auto_cut,
    }))?;

    // this is needed for the auto cut
    printer.send_command(PrinterCommand::SetPageNumber(1))?;

    printer.send_command(PrinterCommand::SetMarginAmount(0))?;

    debug!("printing {} lines", lines.len());

    for line in lines {
        printer.send_command(PrinterCommand::RasterGraphicsTransfer(line))?;
    }

    printer.send_command(PrinterCommand::PrintWithFeeding)?;

    trace!("{:#?}", printer.read_status()?);
    trace!("{:#?}", printer.read_status()?);
    trace!("{:#?}", printer.read_status()?);

    Ok(())
}
