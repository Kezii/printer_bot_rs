# QL-500 Rust Telegram Bot

## About
This project is a Telegram bot developed in Rust, designed to interface directly with the QL-500 thermal printer. It eliminates the need for Python dependencies, typically required by similar projects using libraries like `brother_ql`. Our bot can receive images and stickers, process them by scaling, gamma correction, and dithering, and then print them using the `lp0` Linux driver.

## Features
- **Rust-based**: Written in Rust, a modern, fast, and safe programming language.
- **No Python Dependencies**: Operates independently of Python
- **Image Processing**: Scales, gamma-corrects, and applies dithering to images and stickers.
- **Direct Printing**: Uses the `lp0` Linux driver for direct interfacing with the QL-500 printer.


This readme may or may not have been written by a bot.