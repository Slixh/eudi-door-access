//use anyhow::Result;
//use rppal::gpio::{Gpio, OutputPin};
//use std::time::Duration;
//
//pub struct Door {
//    pin: OutputPin,
//}
//
//impl Door {
//    pub fn new(bcm_pin: u8) -> Result<Self> {
//        let pin = Gpio::new()?.get(bcm_pin)?.into_output_low();
//        Ok(Self { pin })
//    }
//
//    pub async fn unlock_for_secs(&mut self, secs: u64) -> Result<()> {
//        self.pin.set_high();
//        tokio::time::sleep(Duration::from_secs(secs)).await;
//        self.pin.set_low();
//        Ok(())
//    }
//}