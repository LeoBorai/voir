use cacao::appkit::App;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use super::app::VoirApp;

#[derive(Clone, Debug)]
pub enum Message {
    OpenFile(PathBuf),
}

#[derive(Debug, Default)]
pub struct Storage(Rc<RefCell<Vec<u8>>>);

impl Storage {
    pub fn load_bytes(&self, bytes: Vec<u8>) {
        let mut state = self.0.borrow_mut();

        *state = bytes;
    }

    pub fn chunks(&self) -> Vec<Vec<u8>> {
        let bytes = self.0.borrow();

        bytes
            .chunks(16)
            .map(|chunk| chunk.to_vec())
            .collect::<Vec<Vec<u8>>>()
    }

    pub fn len(&self) -> usize {
        let state = self.0.borrow();

        state.len()
    }

    pub fn with<F>(&self, row: usize, mut handler: F)
    where
        F: FnMut(&u8),
    {
        let stack = self.0.borrow();

        if let Some(byte) = stack.get(row) {
            handler(byte);
        }
    }

    pub fn with_byte_mut<F>(&self, row: usize, handler: F)
    where
        F: Fn(&mut u8),
    {
        let mut state = self.0.borrow_mut();

        if let Some(byte) = state.get_mut(row) {
            handler(byte);
        }
    }
}

pub fn dispatch(message: Message) {
    println!("Dispatching Message: {:?}", message);
    App::<VoirApp, Message>::dispatch_main(message);
}
