//! A module that implements common utilities for logging, using slog.
use slog;
use slog::*;
use std::fmt;
use std::io;
use std::io::Write;
use std::panic::RefUnwindSafe;
use std::panic::UnwindSafe;
use std::result;
use std::sync;
use std::time::Instant;

/// Threadsafe timestamp formatting function type
///
/// To satify `slog-rs` thread and unwind safety requirements, the
/// bounds expressed by this trait need to satisfied for a function
/// to be used in timestamp formatting.
pub trait ThreadSafeTimestampFn:
    Fn(&mut io::Write) -> io::Result<()> + Send + Sync + UnwindSafe + RefUnwindSafe + 'static
{
}

impl<F> ThreadSafeTimestampFn for F
where
    F: Fn(&mut io::Write) -> io::Result<()> + Send + Sync,
    F: UnwindSafe + RefUnwindSafe + 'static,
    F: ?Sized,
{
}

pub trait LogPrinter {
    type RecordPrinter: LogRecordPrinter;

    fn with_record<F>(&self, record: &Record, f: F) -> io::Result<()>
    where
        F: FnOnce(Self::RecordPrinter) -> io::Result<()>;
}

pub trait LogRecordPrinter {
    fn print_header(
        &mut self,
        record: &Record,
        fn_timestamp: &ThreadSafeTimestampFn<Output = io::Result<()>>,
    ) -> io::Result<()>;
    fn print_kv<K: fmt::Display, V: fmt::Display>(&mut self, k: K, v: V) -> io::Result<()>;
    fn finish(self) -> io::Result<()>;
}

pub struct ColorPrinter<W: io::Write> {
    w: sync::Arc<sync::Mutex<W>>,
}

impl<W: io::Write> ColorPrinter<W> {
    pub fn new(io: W) -> Self {
        Self {
            w: sync::Arc::new(sync::Mutex::new(io)),
        }
    }
}

impl<W: io::Write> LogPrinter for ColorPrinter<W> {
    type RecordPrinter = ColorRecordPrinter<W>;

    fn with_record<F>(&self, _record: &Record, f: F) -> io::Result<()>
    where
        F: FnOnce(Self::RecordPrinter) -> io::Result<()>,
    {
        f(ColorRecordPrinter {
            io: self.w.clone(),
            buf: Vec::with_capacity(128),
        })
    }
}

pub struct ColorRecordPrinter<W: io::Write> {
    io: sync::Arc<sync::Mutex<W>>,
    buf: Vec<u8>,
}

impl<W: io::Write> LogRecordPrinter for ColorRecordPrinter<W> {
    fn print_header(
        &mut self,
        record: &Record,
        fn_timestamp: &ThreadSafeTimestampFn<Output = io::Result<()>>,
    ) -> io::Result<()> {
        let mut rd = CountingWriter::new(&mut self.buf);

        write!(rd.w, "\x1b[34m")?;
        fn_timestamp(&mut rd)?;
        write!(rd, " ")?;

        let level = record.level();
        match level {
            Level::Critical => write!(rd.w, "\x1b[31m")?,
            Level::Error => write!(rd.w, "\x1b[31m")?,
            Level::Warning => write!(rd.w, "\x1b[33m")?,
            Level::Info => write!(rd.w, "\x1b[32m")?,
            Level::Debug => write!(rd.w, "\x1b[37m")?,
            Level::Trace => write!(rd.w, "\x1b[37m")?,
        };
        write!(rd, "{} ", level.as_short_str())?;

        write!(rd.w, "\x1b[35;1m")?;
        let tag = record.tag();
        if tag.is_empty() {
            write!(rd, "|{}| ", record.module())?;
        } else {
            write!(rd, "|{}| ", tag)?;
        }

        // Write the actual log message. We need to record the size of the message
        // so we use a CountingWriter to avoid an allocation here.
        write!(rd.w, "\x1b[37;1m")?;
        write!(rd, "{}", record.msg())?;
        write!(rd.w, "\x1b[0m")?;
        let msglen = rd.count();
        if msglen < 80 {
            write!(rd.w, "{:.<1$}", "", 80 - msglen)?;
        }
        Ok(())
    }

    fn print_kv<K: fmt::Display, V: fmt::Display>(&mut self, k: K, v: V) -> io::Result<()> {
        write!(&mut self.buf, " {}=", k)?;
        write!(&mut self.buf, "\x1b[37;1m")?;
        write!(&mut self.buf, "{}", v)?;
        write!(&mut self.buf, "\x1b[0m")?;
        Ok(())
    }

    fn finish(mut self) -> io::Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        write!(&mut self.buf, "\n")?;

        let mut io = self
            .io
            .lock()
            .map_err(|_| io::Error::new(io::ErrorKind::Other, "mutex locking error"))?;

        io.write_all(&self.buf)?;
        self.buf.clear();
        io.flush()
    }
}

struct Serializer<'a, RP: LogRecordPrinter> {
    printer: &'a mut RP,
    reverse: bool,
    stack: Vec<(String, String)>,
}

impl<'a, RP: LogRecordPrinter> Serializer<'a, RP> {
    fn new(printer: &'a mut RP, reverse: bool) -> Self {
        Serializer {
            printer: printer,
            reverse: reverse,
            stack: vec![],
        }
    }

    fn finish(mut self) -> io::Result<()> {
        loop {
            if let Some((k, v)) = self.stack.pop() {
                self.printer.print_kv(&k, &v)?;
            } else {
                return Ok(());
            }
        }
    }
}

impl<'a, RP: LogRecordPrinter> Drop for Serializer<'a, RP> {
    fn drop(&mut self) {
        if !self.stack.is_empty() {
            panic!("stack not empty");
        }
    }
}

macro_rules! s(
    ($s:expr, $k:expr, $v:expr) => {
        if $s.reverse {
            $s.stack.push(($k.into(), format!("{}", $v)));
        } else {
            $s.printer.print_kv($k, $v)?;
        }
    };
);

impl<'a, RP: LogRecordPrinter> slog::ser::Serializer for Serializer<'a, RP> {
    fn emit_none(&mut self, key: Key) -> slog::Result {
        s!(self, key, "None");
        Ok(())
    }
    fn emit_unit(&mut self, key: Key) -> slog::Result {
        s!(self, key, "()");
        Ok(())
    }

    fn emit_bool(&mut self, key: Key, val: bool) -> slog::Result {
        s!(self, key, val);
        Ok(())
    }

    fn emit_char(&mut self, key: Key, val: char) -> slog::Result {
        s!(self, key, val);
        Ok(())
    }

    fn emit_usize(&mut self, key: Key, val: usize) -> slog::Result {
        s!(self, key, val);
        Ok(())
    }
    fn emit_isize(&mut self, key: Key, val: isize) -> slog::Result {
        s!(self, key, val);
        Ok(())
    }

    fn emit_u8(&mut self, key: Key, val: u8) -> slog::Result {
        s!(self, key, val);
        Ok(())
    }
    fn emit_i8(&mut self, key: Key, val: i8) -> slog::Result {
        s!(self, key, val);
        Ok(())
    }
    fn emit_u16(&mut self, key: Key, val: u16) -> slog::Result {
        s!(self, key, val);
        Ok(())
    }
    fn emit_i16(&mut self, key: Key, val: i16) -> slog::Result {
        s!(self, key, val);
        Ok(())
    }
    fn emit_u32(&mut self, key: Key, val: u32) -> slog::Result {
        s!(self, key, val);
        Ok(())
    }
    fn emit_i32(&mut self, key: Key, val: i32) -> slog::Result {
        s!(self, key, val);
        Ok(())
    }
    fn emit_f32(&mut self, key: Key, val: f32) -> slog::Result {
        s!(self, key, val);
        Ok(())
    }
    fn emit_u64(&mut self, key: Key, val: u64) -> slog::Result {
        s!(self, key, val);
        Ok(())
    }
    fn emit_i64(&mut self, key: Key, val: i64) -> slog::Result {
        s!(self, key, val);
        Ok(())
    }
    fn emit_f64(&mut self, key: Key, val: f64) -> slog::Result {
        s!(self, key, val);
        Ok(())
    }
    fn emit_str(&mut self, key: Key, val: &str) -> slog::Result {
        s!(self, key, val);
        Ok(())
    }
    fn emit_arguments(&mut self, key: Key, val: &fmt::Arguments) -> slog::Result {
        s!(self, key, val);
        Ok(())
    }
}

// Wrapper for `Write` types that counts total bytes written.
struct CountingWriter<W: io::Write> {
    w: W,
    count: usize,
}

impl<W: io::Write> CountingWriter<W> {
    fn new(w: W) -> Self {
        CountingWriter { w: w, count: 0 }
    }

    fn count(&self) -> usize {
        self.count
    }
}

impl<W: io::Write> io::Write for CountingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.w.write(buf).map(|n| {
            self.count += n;
            n
        })
    }

    fn flush(&mut self) -> io::Result<()> {
        self.w.flush()
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.w.write_all(buf).map(|_| {
            self.count += buf.len();
            ()
        })
    }
}

pub struct LogDrain<RP>
where
    RP: LogPrinter,
{
    printer: RP,
    fn_timestamp: Box<ThreadSafeTimestampFn<Output = io::Result<()>>>,
    use_original_order: bool,
}

pub struct LogDrainBuilder<RP>
where
    RP: LogPrinter,
{
    printer: RP,
    fn_timestamp: Box<ThreadSafeTimestampFn<Output = io::Result<()>>>,
    original_order: bool,
}

impl<RP> LogDrainBuilder<RP>
where
    RP: LogPrinter,
{
    /// Provide a custom function to generate the timestamp
    pub fn use_custom_timestamp<F>(mut self, f: F) -> Self
    where
        F: ThreadSafeTimestampFn,
    {
        self.fn_timestamp = Box::new(f);
        self
    }

    /// Use the original ordering of key-value pairs
    ///
    /// By default, key-values are printed in a reversed order. This option will
    /// change it to the order in which key-values were added.
    pub fn use_original_order(mut self) -> Self {
        self.original_order = true;
        self
    }

    /// Build `FullFormat`
    pub fn build(self) -> LogDrain<RP> {
        LogDrain {
            printer: self.printer,
            fn_timestamp: self.fn_timestamp,
            use_original_order: self.original_order,
        }
    }
}

impl<RP: LogPrinter> Drain for LogDrain<RP> {
    type Ok = ();
    type Err = io::Error;

    fn log(&self, record: &Record, values: &OwnedKVList) -> result::Result<Self::Ok, Self::Err> {
        self.format_full(record, values)
    }
}

impl<RP: LogPrinter> LogDrain<RP> {
    pub fn new(p: RP) -> LogDrainBuilder<RP> {
        let now = Instant::now();
        LogDrainBuilder {
            fn_timestamp: Box::new(move |w: &mut io::Write| -> io::Result<()> {
                write!(w, "[{}]", now.elapsed().as_secs())
            }),
            printer: p,
            original_order: false,
        }
    }

    fn format_full(&self, record: &Record, values: &OwnedKVList) -> io::Result<()> {
        self.printer.with_record(record, |mut printer| {
            printer.print_header(&record, &*self.fn_timestamp)?;
            {
                let mut serializer = Serializer::new(&mut printer, self.use_original_order);
                record.kv().serialize(record, &mut serializer)?;
                values.serialize(record, &mut serializer)?;
                serializer.finish()?;
            }
            printer.finish()
        })
    }
}

pub fn new_console_logger() -> slog::Logger {
    let printer = ColorPrinter::new(std::io::stdout());
    let drain = LogDrain::new(printer).build().fuse();
    slog::Logger::root(drain, o!())
}
