use std::env;

use crate::driver::{PrinterCommand, PrinterCommandMode, PrinterExpandedMode, PrinterMode};
use crate::error::BrotherQlError;
use crate::{driver, Settings};
use image::{ImageBuffer, Luma, Rgba};
use log::{debug, trace};

fn apply_dithering(
    mut input_img: ImageBuffer<Luma<u8>, Vec<u8>>,
) -> Result<ImageBuffer<Rgba<u8>, Vec<u8>>, BrotherQlError> {
    // match the brightness of the previous implementation
    let gamma_correction = 3.14;

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
) -> Result<ImageBuffer<Rgba<u8>, Vec<u8>>, BrotherQlError> {
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

fn img_to_lines(
    img: ImageBuffer<Rgba<u8>, Vec<u8>>,
    image_width: u32,
) -> Result<Vec<[u8; 90]>, BrotherQlError> {
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
    let padding = 720 - image_width;

    for y in 0..img.height() {
        let mut line = [0u8; 90];

        for x in 0..img.width() {
            let i = img.get_pixel(x, y).0[0];
            let x = x + padding;
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

pub fn render_image(file_path: &str, settings: &Settings) -> Result<Vec<[u8; 90]>, BrotherQlError> {
    use image::ImageReader;

    let img = ImageReader::open(file_path)?.decode()?;

    // Limit stickers ratio (so people don't print incredibly long stickers)

    let ratio = img.height() as f32 / img.width() as f32;

    if ratio > 3.5 {
        println!("Ratio is too high: {}", ratio);
        return Err(BrotherQlError::InvalidImage);
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

    // let new_width = 720; //630 per la carta piccola
    let mut printer = driver::PrinterCommander::main("/dev/usb/lp0")?;
    let status = printer.read_status()?;
    let new_width = status.pixel_width().unwrap_or(720) as u32;
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

    // if the paper format is not known, assume the biggest one
    let lines = img_to_lines(dithered_img, new_width)?;
    Ok(lines)
}

pub fn print_lines(lines: Vec<[u8; 90]>, settings: &Settings) -> Result<(), BrotherQlError> {
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
