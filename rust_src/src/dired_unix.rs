use libc::{c_char, c_long, endpwent, getgrgid, getpwent, getpwuid, group, passwd,
           timespec as c_timespec};

use std::ffi::{CStr, CString, OsStr};
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::slice;

use remacs_sys::{build_string, file_attributes_c_internal, filemode_string, globals,
                 Fexpand_file_name, Ffind_file_name_handler, Qfile_attributes, Qnil};

use lisp::LispObject;
use lists::list;
use time::make_lisp_time;

trait StringExt {
    // LispObject strings should use build_string for correct GC behavior.
    fn to_bstring(&self) -> LispObject;
    fn to_cstring(&self) -> *const c_char;
    fn to_dir_f(&self) -> (String, String);
}

impl StringExt for String {
    fn to_bstring(&self) -> LispObject {
        let c_str = CString::new(self.as_str()).unwrap();
        unsafe { build_string(c_str.as_ptr() as *const i8) }
    }
    fn to_cstring(&self) -> *const c_char {
        let c_str = CString::new(self.as_str()).unwrap();
        (c_str.as_ptr() as *const c_char)
    }
    // Split-up absolute path to directory and file
    fn to_dir_f(&self) -> (String, String) {
        let path = Path::new(self.as_str());
        let parent: &Path;
        match path.parent() {
            None => parent = Path::new(""),
            Some(p) => parent = p,
        }
        let stem: &OsStr;
        match path.file_stem() {
            None => stem = OsStr::new(""),
            Some(s) => stem = s,
        }
        let ext: &OsStr;
        match path.extension() {
            None => ext = OsStr::new(""),
            Some(s) => ext = s,
        }

        let dir_s: String;
        let stem_s: String;
        let ext_s: String;
        match parent.to_str() {
            None => error!("new parent path is not a valid UTF-8 sequence"),
            Some(s) => dir_s = s.to_string(),
        }
        match stem.to_str() {
            None => error!("new stem of path is not a valid UTF-8 sequence"),
            Some(s) => stem_s = s.to_string(),
        }
        match ext.to_str() {
            None => error!("new extension of path is not a valid UTF-8 sequence"),
            Some(s) => ext_s = s.to_string(),
        }

        if ext_s.is_empty() {
            (dir_s, stem_s)
        } else {
            (dir_s, stem_s + "." + &ext_s)
        }
    }
}

trait LispObjectExt {
    fn to_idfstring(&self) -> String;
    fn to_stdstring(&self) -> String;
}

impl LispObjectExt for LispObject {
    fn to_idfstring(&self) -> String {
        if self.is_nil() {
            "NOTstring".to_string()
        } else {
            let idf_sym_s = self.symbol_or_string_as_string();
            idf_sym_s.to_string().to_lowercase()
        }
    }
    fn to_stdstring(&self) -> String {
        let s = self.as_string().unwrap(); //LispObject String
        let slice = unsafe { slice::from_raw_parts(s.const_data_ptr(), s.len_bytes() as usize) };
        String::from_utf8_lossy(slice).into_owned()
    }
}

struct FileAttrs {
    use_c_internal: bool, // escape hatch
    fpath: String,
    ftype_is_sym: bool,
    ftype_sym_path: String,
    ftype_is_dir: bool,
    nlinks: u64,
    id_format: String,
    idf_is_int: bool,
    idf_u_is_int: bool, // fallback if pw lookup fails
    idf_g_is_int: bool, // fallback if gr lookup fails
    idf_uid: u32,
    idf_gid: u32,
    idf_uname: String,
    idf_gname: String,
    atime_s: i64,
    atime_ns: c_long,
    mtime_s: i64,
    mtime_ns: c_long,
    ctime_s: i64,
    ctime_ns: c_long,
    size: u64,
    //file_mode: String,
    ino: u64,
    dev: u64,
}

impl FileAttrs {
    fn new(fpath: String, id_format: String) -> Self {
        Self {
            use_c_internal: false,
            fpath,
            ftype_is_sym: false,
            ftype_sym_path: "deadbeef".to_string(),
            ftype_is_dir: false,
            nlinks: 0,
            id_format,
            idf_is_int: true,
            idf_u_is_int: false,
            idf_g_is_int: false,
            idf_uid: 65534, // 'nobody' on Ubuntu
            idf_gid: 65534, // 'nogroup' on Ubuntu
            idf_uname: "deadbeef".to_string(),
            idf_gname: "deadbeef".to_string(),
            atime_s: 0,
            atime_ns: 0,
            mtime_s: 0,
            mtime_ns: 0,
            ctime_s: 0,
            ctime_ns: 0,
            size: 0,
            //file_mode,
            ino: 0,
            dev: 0,
        }
    }

    fn get(&mut self) -> io::Result<()> {
        let md = fs::metadata(self.fpath.clone())?;

        // file_type
        let ft = md.file_type();
        // is_symlink() appears to be always -- somewhat counter intuitively --
        // false cuz the link is followed first.
        if let Ok(symmemaybe) = fs::read_link(self.fpath.clone()) {
            self.ftype_is_sym = true;
            self.ftype_sym_path = symmemaybe.to_str().unwrap().to_string();

            // Punt back to C for symlinks as the Rust trait
            // std::os::fs::symlink_metadata does not provide most
            // of the fields needed.
            // std::os::fs::metadata does not work cuz it follows the link first.
            self.use_c_internal = true;

            return Ok(());
        } else {
            if ft.is_dir() {
                self.ftype_is_dir = true;
            } else {
                self.ftype_is_dir = false;
            }
        }

        self.nlinks = md.nlink();

        //  2. File uid as a string or a number.  If a string value cannot be
        //  looked up, a numeric value, either an integer or a float, is returned.
        self.idf_is_int = !("string".to_owned() == self.id_format);
        if self.idf_is_int {
            self.idf_uid = md.uid();
            self.idf_gid = md.gid();
        } else {
            let pw: *mut passwd = unsafe { getpwuid(md.uid()) };
            if pw.is_null() {
                self.idf_u_is_int = true;
                self.idf_uid = md.uid();
            } else {
                let c_buf: *const c_char = unsafe { (*pw).pw_name };
                let c_str: &CStr = unsafe { CStr::from_ptr(c_buf) };
                let str_slice: &str = c_str.to_str().unwrap();
                self.idf_uname = str_slice.to_owned();
            }

            let gr: *mut group = unsafe { getgrgid(md.gid()) };
            if gr.is_null() {
                self.idf_gid = md.gid();
                self.idf_g_is_int = true;
            } else {
                let c_buf: *const c_char = unsafe { (*gr).gr_name };
                let c_str: &CStr = unsafe { CStr::from_ptr(c_buf) };
                let str_slice: &str = c_str.to_str().unwrap();
                self.idf_gname = str_slice.to_owned();
            }
        }

        self.atime_s = md.atime();
        self.atime_ns = c_long::from(md.atime_nsec());
        self.mtime_s = md.mtime();
        self.mtime_ns = c_long::from(md.mtime_nsec());
        self.ctime_s = md.ctime();
        self.ctime_ns = c_long::from(md.ctime_nsec());

        self.size = md.size();

        self.ino = md.ino();
        self.dev = md.dev();

        Ok(())
    }

    // FileAttrs -> LispObject list
    fn to_list(&self) -> LispObject {
        if self.use_c_internal {
            let (dir, f) = self.fpath.to_dir_f();
            let name = CString::new(self.fpath.clone().as_str()).unwrap();
            return unsafe {
                file_attributes_c_internal(
                    name.as_ptr(),
                    LispObject::from(dir.as_str()),
                    LispObject::from(f.as_str()),
                    LispObject::from(self.id_format.as_str()),
                )
            };
        }

        let mut attrs = Vec::new();

        //  0. t for directory, string (name linked to) for symbolic link, or nil.
        if self.ftype_is_sym {
            attrs.push(self.ftype_sym_path.to_owned().to_bstring());
        } else {
            if self.ftype_is_dir {
                attrs.push(LispObject::constant_t());
            } else {
                attrs.push(LispObject::constant_nil());
            }
        }

        //  1. Number of links to file.
        attrs.push(LispObject::from_natnum(self.nlinks));

        //  2. File uid as a string or a number.  If a string value cannot be
        //     looked up, a numeric value, either an integer or a float, is returned.
        //  3. File gid, likewise.
        if self.idf_is_int || self.idf_u_is_int {
            attrs.push(LispObject::from_natnum(u64::from(self.idf_uid)));
        } else {
            attrs.push(self.idf_uname.to_owned().to_bstring());
        }
        if self.idf_is_int || self.idf_g_is_int {
            attrs.push(LispObject::from_natnum(u64::from(self.idf_gid)));
        } else {
            attrs.push(self.idf_gname.to_owned().to_bstring());
        }

        //  4. Last access time, as a list of integers (HIGH LOW USEC PSEC) in the
        //     same style as (current-time).
        //     (See a note below about access time on FAT-based filesystems.)
        //  5. Last modification time, likewise.  This is the time of the last
        //     change to the file's contents.
        //  6. Last status change time, likewise.  This is the time of last change
        //     to the file's attributes: owner and group, access mode bits, etc.
        attrs.push(make_lisp_time(c_timespec {
            tv_sec: self.atime_s,
            tv_nsec: self.atime_ns, //tv_nsec: self.atime_ns
        }));
        attrs.push(make_lisp_time(c_timespec {
            tv_sec: self.mtime_s,
            tv_nsec: self.mtime_ns,
        }));
        attrs.push(make_lisp_time(c_timespec {
            tv_sec: self.ctime_s,
            tv_nsec: self.ctime_ns,
        }));

        //  7. Size in bytes.
        //     This is a floating point number if the size is too large for an integer.
        //     remacs: symlink size is of file linked to? (or size of path str?)
        attrs.push(LispObject::from_natnum(self.size));

        //  8. File modes, as a string of ten letters or dashes as in ls -l.
        let fpath_lo = LispObject::from(self.fpath.as_str());
        //  Punt back to C until the filemode_string code is ported to Rust.
        attrs.push(unsafe { filemode_string(fpath_lo) });

        //  9. An unspecified value, present only for backward compatibility.
        attrs.push(LispObject::constant_t());

        // 10. inode number.  If it is larger than what an Emacs integer can hold,
        //     this is of the form (HIGH . LOW): first the high bits, then the low 16 bits.
        //     If even HIGH is too large for an Emacs integer, this is instead of the form
        //     (HIGH MIDDLE . LOW): first the high bits, then the middle 24 bits,
        //     and finally the low 16 bits.
        attrs.push(LispObject::from_natnum(self.ino));

        // 11. Filesystem device number.  If it is larger than what the Emacs
        //     integer can hold, this is a cons cell, similar to the inode number.
        attrs.push(LispObject::from_natnum(self.dev));

        list(&mut attrs)
    }
}

pub fn file_attributes_intro(filename: LispObject, id_format: LispObject) -> LispObject {
    let fnexp = unsafe { Fexpand_file_name(filename.to_raw(), Qnil) };
    let handler = unsafe { Ffind_file_name_handler(fnexp, Qfile_attributes) };
    if handler.is_not_nil() {
        if id_format.is_not_nil() {
            return call!(handler, Qfile_attributes, fnexp, id_format);
        } else {
            return call!(handler, Qfile_attributes, fnexp);
        }
    }

    file_attributes_core(fnexp, id_format)
}

pub fn file_attributes_rust_internal2(
    dirname: LispObject,
    filename: LispObject,
    id_format: LispObject,
) -> LispObject {
    let fpath_s = dirname.to_stdstring() + "/" + &filename.to_stdstring();
    let fpath = LispObject::from(fpath_s.as_str());

    file_attributes_core(fpath, id_format)
}

fn file_attributes_core(fpath: LispObject, id_format: LispObject) -> LispObject {
    let mut attrs = FileAttrs::new(fpath.to_stdstring(), id_format.to_idfstring());
    let res = attrs.get();
    if res.is_err() {
        Qnil
    } else {
        attrs.to_list()
    }
}

fn get_user_real_login_name() -> LispObject {
    unsafe { globals.Vuser_real_login_name }
}

pub fn get_users() -> LispObject {
    let mut done = false;
    let mut unames = Vec::new();

    while !done {
        let pw: *mut passwd = unsafe { getpwent() };
        if pw.is_null() {
            done = true;
        } else {
            unames.push(unsafe { build_string((*pw).pw_name) })
        }
    }
    unsafe { endpwent() };

    if unames.is_empty() {
        unames.push(get_user_real_login_name());
    }

    list(&mut unames)
}
