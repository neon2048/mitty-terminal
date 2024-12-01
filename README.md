# mitty-terminal

Mitty-terminal is a work in progress hardware device for displaying update board messages from https://mitty-terminal.uwu.ai/ on a small ST7735 display using an ESP32S3.

## Project state

Right now, this is an extremely barebones proof of concept, all it can do is:
* connect to Wifi
* download https://mitty-terminal.uwu.ai/
* parse the HTML, extract update board messages
* display the first message on the screen.

### Wiring
* ST7735 <-> ESP32S3
* LED - 3V3
* CLK - GPIO36 (SPI3 CLK)
* SDA (MISO) - GPIO35 (SPI3 MOSI)
* RS (A0) - GPIO2 
* RST - GPIO41 
* CS - GPIO39 (SPI3 CS)
* GND - GND
* VCC - 3V3
