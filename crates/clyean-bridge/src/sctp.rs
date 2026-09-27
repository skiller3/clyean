// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! One-to-one style SCTP sockets over the kernel's SCTP stack, through `libc` rather than
//! `libsctp`, because the container end of the bridge is a static executable.  Messages
//! keep their stream number and payload protocol identifier, which travel in `sendmsg`
//! and `recvmsg` ancillary data; multi-homing is not used.

use std::io;
use std::mem::{size_of, zeroed};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::time::Duration;

use crate::protocol::{Message, MAX_MESSAGE};

// From <linux/sctp.h>.
const SOL_SCTP: libc::c_int = 132;
const SCTP_RECVRCVINFO: libc::c_int = 32;
const SCTP_SNDINFO: libc::c_int = 2;
const SCTP_RCVINFO: libc::c_int = 3;
const MSG_NOTIFICATION: libc::c_int = 0x8000;

#[repr(C)]
#[derive(Default)]
struct SndInfo {
    sid: u16,
    flags: u16,
    ppid: u32,
    context: u32,
    assoc_id: i32,
}

#[repr(C)]
#[derive(Default)]
struct RcvInfo {
    sid: u16,
    ssn: u16,
    flags: u16,
    ppid: u32,
    tsn: u32,
    cumtsn: u32,
    context: u32,
    assoc_id: i32,
}

/// Whether this kernel can create SCTP sockets (it may need to load its SCTP module).
pub fn is_available() -> bool {
    socket(IpAddr::V4(Ipv4Addr::LOCALHOST)).is_ok()
}

fn socket(address: IpAddr) -> io::Result<OwnedFd> {
    let family = match address {
        IpAddr::V4(_) => libc::AF_INET,
        IpAddr::V6(_) => libc::AF_INET6,
    };
    // SAFETY: socket either fails or returns a descriptor this function takes ownership of.
    let fd = unsafe {
        libc::socket(
            family,
            libc::SOCK_STREAM | libc::SOCK_CLOEXEC,
            libc::IPPROTO_SCTP,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fd is a freshly created, open descriptor owned by nothing else.
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    set_option(fd.as_raw_fd(), SOL_SCTP, SCTP_RECVRCVINFO, 1)?;
    Ok(fd)
}

fn set_option(
    fd: RawFd,
    level: libc::c_int,
    name: libc::c_int,
    value: libc::c_int,
) -> io::Result<()> {
    // SAFETY: setsockopt reads exactly size_of::<c_int>() bytes from `value`.
    let result = unsafe {
        libc::setsockopt(
            fd,
            level,
            name,
            (&value as *const libc::c_int).cast(),
            size_of::<libc::c_int>() as libc::socklen_t,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn set_timeout(fd: RawFd, name: libc::c_int, timeout: Option<Duration>) -> io::Result<()> {
    let timeout = timeout.unwrap_or_default();
    // The field types are inferred rather than named: musl deprecates the aliases.
    // SAFETY: an all-zero timeval is valid.
    let mut value: libc::timeval = unsafe { zeroed() };
    value.tv_sec = timeout.as_secs() as _;
    value.tv_usec = timeout.subsec_micros() as _;
    // SAFETY: setsockopt reads exactly size_of::<timeval>() bytes from `value`.
    let result = unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            name,
            (&value as *const libc::timeval).cast(),
            size_of::<libc::timeval>() as libc::socklen_t,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// A socket address in the C layout the kernel expects, and its length.
fn raw_address(address: SocketAddr) -> (libc::sockaddr_storage, libc::socklen_t) {
    // SAFETY: an all-zero sockaddr_storage is valid, and each arm writes a complete
    // sockaddr of the matching family into its prefix.
    unsafe {
        let mut storage: libc::sockaddr_storage = zeroed();
        let length = match address {
            SocketAddr::V4(v4) => {
                let raw =
                    &mut *(&mut storage as *mut libc::sockaddr_storage).cast::<libc::sockaddr_in>();
                raw.sin_family = libc::AF_INET as libc::sa_family_t;
                raw.sin_port = v4.port().to_be();
                raw.sin_addr = libc::in_addr {
                    s_addr: u32::from_ne_bytes(v4.ip().octets()),
                };
                size_of::<libc::sockaddr_in>()
            }
            SocketAddr::V6(v6) => {
                let raw = &mut *(&mut storage as *mut libc::sockaddr_storage)
                    .cast::<libc::sockaddr_in6>();
                raw.sin6_family = libc::AF_INET6 as libc::sa_family_t;
                raw.sin6_port = v6.port().to_be();
                raw.sin6_addr = libc::in6_addr {
                    s6_addr: v6.ip().octets(),
                };
                raw.sin6_scope_id = v6.scope_id();
                size_of::<libc::sockaddr_in6>()
            }
        };
        (storage, length as libc::socklen_t)
    }
}

/// A listening SCTP socket on one address.
pub struct Listener {
    fd: OwnedFd,
}

impl Listener {
    /// Binds and listens, which holds the port; associations wait until `accept`.
    pub fn bind(address: SocketAddr) -> io::Result<Self> {
        let fd = socket(address.ip())?;
        set_option(fd.as_raw_fd(), libc::SOL_SOCKET, libc::SO_REUSEADDR, 1)?;
        if address.is_ipv6() {
            set_option(fd.as_raw_fd(), libc::IPPROTO_IPV6, libc::IPV6_V6ONLY, 1)?;
        }
        let (raw, length) = raw_address(address);
        // SAFETY: raw holds a valid sockaddr of `length` bytes.
        let result = unsafe {
            libc::bind(
                fd.as_raw_fd(),
                (&raw as *const libc::sockaddr_storage).cast(),
                length,
            )
        };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: listen on an open, bound descriptor.
        if unsafe { libc::listen(fd.as_raw_fd(), 128) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { fd })
    }

    /// Makes `accept` wait at most `poll` for an association, so that a caller can notice
    /// it should stop.
    pub fn set_accept_poll(&self, poll: Duration) -> io::Result<()> {
        set_timeout(self.fd.as_raw_fd(), libc::SO_RCVTIMEO, Some(poll))
    }

    /// The next association, or `None` when none arrived within the poll interval.
    pub fn accept(&self) -> io::Result<Option<Association>> {
        // SAFETY: accept4 with null address arguments only returns a new descriptor.
        let fd = unsafe {
            libc::accept4(
                self.fd.as_raw_fd(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                libc::SOCK_CLOEXEC,
            )
        };
        if fd < 0 {
            let error = io::Error::last_os_error();
            return match error.kind() {
                io::ErrorKind::WouldBlock
                | io::ErrorKind::TimedOut
                | io::ErrorKind::Interrupted => Ok(None),
                _ => Err(error),
            };
        }
        // SAFETY: fd is a freshly accepted descriptor owned by nothing else.
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        set_option(fd.as_raw_fd(), SOL_SCTP, SCTP_RECVRCVINFO, 1)?;
        Ok(Some(Association { fd }))
    }
}

/// One SCTP association.
pub struct Association {
    fd: OwnedFd,
}

impl Association {
    /// Connects to `port` on the loopback, IPv4 first, then IPv6.
    pub fn connect_loopback(port: u16, timeout: Duration) -> Option<Self> {
        [
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V6(Ipv6Addr::LOCALHOST),
        ]
        .into_iter()
        .find_map(|ip| Self::connect(SocketAddr::new(ip, port), timeout).ok())
    }

    fn connect(address: SocketAddr, timeout: Duration) -> io::Result<Self> {
        let fd = socket(address.ip())?;
        set_timeout(fd.as_raw_fd(), libc::SO_SNDTIMEO, Some(timeout))?;
        let (raw, length) = raw_address(address);
        // SAFETY: raw holds a valid sockaddr of `length` bytes.
        let result = unsafe {
            libc::connect(
                fd.as_raw_fd(),
                (&raw as *const libc::sockaddr_storage).cast(),
                length,
            )
        };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        set_timeout(fd.as_raw_fd(), libc::SO_SNDTIMEO, None)?;
        Ok(Self { fd })
    }

    /// The next whole message, or `None` once the peer has shut the association down.
    pub fn receive(&self) -> io::Result<Option<Message>> {
        let mut payload = Vec::new();
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            let mut info = RcvInfo::default();
            let (received, flags, has_info) = self.receive_part(&mut buffer, &mut info)?;
            if received == 0 && flags & libc::MSG_EOR == 0 && payload.is_empty() {
                return Ok(None);
            }
            if flags & MSG_NOTIFICATION != 0 {
                continue;
            }
            payload.extend_from_slice(&buffer[..received]);
            if payload.len() > MAX_MESSAGE {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("an SCTP message is longer than {MAX_MESSAGE} bytes"),
                ));
            }
            if flags & libc::MSG_EOR != 0 {
                let (stream_number, payload_protocol) = if has_info {
                    (info.sid, info.ppid)
                } else {
                    (0, 0)
                };
                return Ok(Some(Message {
                    stream_number,
                    payload_protocol,
                    payload,
                }));
            }
        }
    }

    fn receive_part(
        &self,
        buffer: &mut [u8],
        info: &mut RcvInfo,
    ) -> io::Result<(usize, libc::c_int, bool)> {
        let mut control = [0u8; 64];
        let mut vector = libc::iovec {
            iov_base: buffer.as_mut_ptr().cast(),
            iov_len: buffer.len(),
        };
        // SAFETY: the message header points at buffers that outlive the call, with their
        // true lengths, and the control messages read back are bounds-checked by CMSG_*.
        unsafe {
            let mut header: libc::msghdr = zeroed();
            header.msg_iov = &mut vector;
            header.msg_iovlen = 1;
            header.msg_control = control.as_mut_ptr().cast();
            header.msg_controllen = control.len() as _;
            let received = loop {
                let received = libc::recvmsg(self.fd.as_raw_fd(), &mut header, 0);
                if received >= 0 {
                    break received as usize;
                }
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::Interrupted {
                    return Err(error);
                }
            };
            let mut has_info = false;
            let mut cmsg = libc::CMSG_FIRSTHDR(&header);
            while !cmsg.is_null() {
                if (*cmsg).cmsg_level == SOL_SCTP && (*cmsg).cmsg_type == SCTP_RCVINFO {
                    std::ptr::copy_nonoverlapping(
                        libc::CMSG_DATA(cmsg),
                        (info as *mut RcvInfo).cast::<u8>(),
                        size_of::<RcvInfo>(),
                    );
                    has_info = true;
                }
                cmsg = libc::CMSG_NXTHDR(&header, cmsg);
            }
            Ok((received, header.msg_flags, has_info))
        }
    }

    /// Sends one message with its stream number and payload protocol identifier.
    pub fn send(&self, message: &Message) -> io::Result<()> {
        let info = SndInfo {
            sid: message.stream_number,
            ppid: message.payload_protocol,
            ..SndInfo::default()
        };
        let mut control = [0u8; 64];
        let mut vector = libc::iovec {
            iov_base: message.payload.as_ptr() as *mut libc::c_void,
            iov_len: message.payload.len(),
        };
        // SAFETY: the header points at buffers that outlive the call; the one control
        // message is written through CMSG_* within the buffer's CMSG_SPACE.
        unsafe {
            let space = libc::CMSG_SPACE(size_of::<SndInfo>() as u32) as usize;
            let mut header: libc::msghdr = zeroed();
            header.msg_iov = &mut vector;
            header.msg_iovlen = 1;
            header.msg_control = control.as_mut_ptr().cast();
            header.msg_controllen = space as _;
            let cmsg = libc::CMSG_FIRSTHDR(&header);
            (*cmsg).cmsg_level = SOL_SCTP;
            (*cmsg).cmsg_type = SCTP_SNDINFO;
            (*cmsg).cmsg_len = libc::CMSG_LEN(size_of::<SndInfo>() as u32) as _;
            std::ptr::copy_nonoverlapping(
                (&info as *const SndInfo).cast::<u8>(),
                libc::CMSG_DATA(cmsg),
                size_of::<SndInfo>(),
            );
            loop {
                if libc::sendmsg(self.fd.as_raw_fd(), &header, libc::MSG_NOSIGNAL) >= 0 {
                    return Ok(());
                }
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::Interrupted {
                    return Err(error);
                }
            }
        }
    }

    /// Ends the association in both directions, unblocking its readers.
    pub fn shutdown(&self) {
        // SAFETY: shutdown on an open descriptor.
        unsafe {
            libc::shutdown(self.fd.as_raw_fd(), libc::SHUT_RDWR);
        }
    }

    /// Sends no more messages; the peer receives the end of the association.
    pub fn shutdown_write(&self) {
        // SAFETY: shutdown on an open descriptor.
        unsafe {
            libc::shutdown(self.fd.as_raw_fd(), libc::SHUT_WR);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_control_structures_match_the_kernel_layout() {
        assert_eq!(size_of::<SndInfo>(), 16);
        assert_eq!(size_of::<RcvInfo>(), 28);
    }

    /// Opt-in with `CLYEAN_SCTP_TESTS=1`: creating the first SCTP socket can make the
    /// kernel load its SCTP module, which changes the machine's state.
    #[test]
    fn messages_keep_their_stream_numbers_and_payload_protocols() {
        if std::env::var_os("CLYEAN_SCTP_TESTS").is_none() {
            eprintln!("skipped: set CLYEAN_SCTP_TESTS=1 to exercise the kernel's SCTP");
            return;
        }
        assert!(is_available(), "this kernel has no SCTP");
        let listener = Listener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).unwrap();
        let port = local_port(&listener.fd);
        listener.set_accept_poll(Duration::from_secs(2)).unwrap();
        let client = std::thread::spawn(move || {
            let association = Association::connect_loopback(port, Duration::from_secs(2)).unwrap();
            association
                .send(&Message {
                    stream_number: 1,
                    payload_protocol: 46,
                    payload: b"setup".to_vec(),
                })
                .unwrap();
            association.receive().unwrap()
        });
        let accepted = listener.accept().unwrap().expect("an association");
        let message = accepted.receive().unwrap().expect("a message");
        assert_eq!((message.stream_number, message.payload_protocol), (1, 46));
        assert_eq!(message.payload, b"setup");
        accepted.send(&Message::datagram(b"ok".to_vec())).unwrap();
        let reply = client.join().unwrap().expect("a reply");
        assert_eq!(reply.payload, b"ok");
    }

    fn local_port(fd: &OwnedFd) -> u16 {
        // SAFETY: getsockname writes at most `length` bytes into the storage.
        unsafe {
            let mut storage: libc::sockaddr_storage = zeroed();
            let mut length = size_of::<libc::sockaddr_storage>() as libc::socklen_t;
            libc::getsockname(
                fd.as_raw_fd(),
                (&mut storage as *mut libc::sockaddr_storage).cast(),
                &mut length,
            );
            let raw = &*(&storage as *const libc::sockaddr_storage).cast::<libc::sockaddr_in>();
            u16::from_be(raw.sin_port)
        }
    }
}
