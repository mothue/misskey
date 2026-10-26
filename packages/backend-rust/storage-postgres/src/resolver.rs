//! Original-operation resolver observations. Diagnostics never contain a host.

use std::{
    fmt, io,
    net::{IpAddr, SocketAddr},
};

/// These are Linux getaddrinfo facts, not a portable errno or Node error code.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct ResolverFacts {
    pub(crate) eai: i32,
    pub(crate) errno: Option<i32>,
    pub(crate) flags: i32,
    pub(crate) family: i32,
    pub(crate) socket_type: i32,
    pub(crate) protocol: i32,
    pub(crate) service: &'static str,
}

impl fmt::Debug for ResolverFacts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ResolverFacts")
    }
}

struct ResolverFailure(ResolverFacts);

impl fmt::Debug for ResolverFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ResolverFailure")
    }
}
impl fmt::Display for ResolverFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("resolver unavailable")
    }
}
impl std::error::Error for ResolverFailure {}

pub(crate) fn facts(error: &io::Error) -> Option<&ResolverFacts> {
    error
        .get_ref()?
        .downcast_ref::<ResolverFailure>()
        .map(|failure| &failure.0)
}

/// Numeric routes bypass DNS. Linux scoped IPv6 resolves only an interface
/// index, preserving its scope rather than normalizing it through IpAddr.
pub(crate) fn numeric_address(host: &str, port: u16) -> Option<io::Result<SocketAddr>> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Some(Ok(SocketAddr::new(ip, port)));
    }
    #[cfg(target_os = "linux")]
    {
        linux::scoped_address(host, port)
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

pub(crate) async fn lookup(host: &str, port: u16) -> io::Result<Vec<SocketAddr>> {
    #[cfg(target_os = "linux")]
    {
        linux::lookup(host, port).await
    }
    #[cfg(not(target_os = "linux"))]
    {
        // Other platforms retain Tokio's resolver behavior. Its io::Error does
        // not establish the Linux EAI/errno facts represented above.
        Ok(tokio::net::lookup_host((host, port)).await?.collect())
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::{ResolverFacts, ResolverFailure};
    use std::{
        ffi::{CStr, CString},
        fmt, io, mem,
        net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6},
        ptr,
    };

    const SERVICE: &CStr = c"0";

    pub(super) fn scoped_address(host: &str, port: u16) -> Option<io::Result<SocketAddr>> {
        let (ip, zone) = host.rsplit_once('%')?;
        let ip = ip.parse::<Ipv6Addr>().ok()?;
        if zone.is_empty() {
            return None;
        }
        let invalid_scope =
            || io::Error::new(io::ErrorKind::InvalidInput, "invalid numeric address scope");
        let scope_id = if zone.bytes().all(|byte| byte.is_ascii_digit()) {
            match zone.parse::<u32>() {
                Ok(scope_id) => scope_id,
                Err(_) => return Some(Err(invalid_scope())),
            }
        } else {
            let zone = match CString::new(zone) {
                Ok(zone) => zone,
                Err(_) => return Some(Err(invalid_scope())),
            };
            // SAFETY: the owned interface name is NUL terminated and remains
            // live throughout this synchronous local interface-index query.
            let index = unsafe { libc::if_nametoindex(zone.as_ptr()) };
            if index == 0 {
                // SAFETY: capture this thread's failure errno immediately.
                let errno = unsafe { *libc::__errno_location() };
                return Some(Err(io::Error::new(
                    io::Error::from_raw_os_error(errno).kind(),
                    "numeric address scope unavailable",
                )));
            }
            index
        };
        Some(Ok(SocketAddr::V6(SocketAddrV6::new(ip, port, 0, scope_id))))
    }

    pub(super) async fn lookup(host: &str, port: u16) -> io::Result<Vec<SocketAddr>> {
        // The owned input lives on the blocking worker until libc has returned.
        // An embedded NUL is an input error, never a manufactured EAI status.
        let host = CString::new(host)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid resolver input"))?;
        blocking_lookup(move || {
            // SAFETY: libc owns a successful, valid addrinfo chain until the
            // matching libc freeaddrinfo; all access remains on this worker.
            unsafe {
                lookup_with(
                    &host,
                    port,
                    |host, service, hints, result| {
                        libc::getaddrinfo(host.as_ptr(), service.as_ptr(), hints, result)
                    },
                    |result| libc::freeaddrinfo(result),
                    || *libc::__errno_location(),
                )
            }
        })
        .await
    }

    struct WorkerFailure;
    impl fmt::Debug for WorkerFailure {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("ResolverWorkerFailure")
        }
    }
    impl fmt::Display for WorkerFailure {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("resolver worker unavailable")
        }
    }
    impl std::error::Error for WorkerFailure {}

    async fn blocking_lookup<F>(work: F) -> io::Result<Vec<SocketAddr>>
    where
        F: FnOnce() -> io::Result<Vec<SocketAddr>> + Send + 'static,
    {
        // Dropping the waiter does not cancel libc. The worker only resolves
        // and frees addresses; opening a socket remains the waiter's work.
        tokio::task::spawn_blocking(work)
            .await
            .map_err(|_| io::Error::other(WorkerFailure))?
    }

    struct AddressList<F: FnOnce(*mut libc::addrinfo)> {
        head: *mut libc::addrinfo,
        free: Option<F>,
    }
    impl<F: FnOnce(*mut libc::addrinfo)> Drop for AddressList<F> {
        fn drop(&mut self) {
            if !self.head.is_null()
                && let Some(free) = self.free.take()
            {
                free(self.head);
            }
        }
    }

    /// A callback seam for deterministic status/ownership tests, not DNS data.
    ///
    /// # Safety
    /// On success `call` must supply a valid, acyclic native addrinfo chain and
    /// address storage of at least each advertised length. It must remain valid
    /// until `free` releases that same head once, including during unwinding.
    /// On failure the out-pointer is unspecified and will not be inspected.
    unsafe fn lookup_with<C, F, E>(
        host: &CStr,
        port: u16,
        call: C,
        free: F,
        errno: E,
    ) -> io::Result<Vec<SocketAddr>>
    where
        C: FnOnce(&CStr, &CStr, &libc::addrinfo, &mut *mut libc::addrinfo) -> i32,
        F: FnOnce(*mut libc::addrinfo),
        E: FnOnce() -> i32,
    {
        // SAFETY: zero is valid for all addrinfo scalar and pointer fields.
        let mut hints: libc::addrinfo = unsafe { mem::zeroed() };
        hints.ai_family = libc::AF_UNSPEC;
        hints.ai_socktype = libc::SOCK_STREAM;
        hints.ai_protocol = 0;
        // Pinned Bun net supplies ADDRCONFIG to its Linux system lookup, whose
        // native service defaults to "0". The configured port is applied below.
        hints.ai_flags = libc::AI_ADDRCONFIG;
        let mut head = ptr::null_mut();
        let status = call(host, SERVICE, &hints, &mut head);
        // errno belongs to this worker and is meaningful only for EAI_SYSTEM.
        // Capture it before allocation, formatting, another FFI call or await.
        let system_errno = if status == libc::EAI_SYSTEM {
            Some(errno())
        } else {
            None
        };
        if status != 0 {
            let kind = system_errno
                .map(|errno| io::Error::from_raw_os_error(errno).kind())
                .unwrap_or(io::ErrorKind::Other);
            return Err(io::Error::new(
                kind,
                ResolverFailure(ResolverFacts {
                    eai: status,
                    errno: system_errno,
                    flags: hints.ai_flags,
                    family: hints.ai_family,
                    socket_type: hints.ai_socktype,
                    protocol: hints.ai_protocol,
                    service: "0",
                }),
            ));
        }

        // The guard is installed before allocation or traversal. Only copied
        // SocketAddrs can leave this worker, even if its waiter was dropped.
        let list = AddressList {
            head,
            free: Some(free),
        };
        let mut addresses = Vec::new();
        let mut next = list.head;
        while !next.is_null() {
            // SAFETY: the call contract keeps every chain entry alive until
            // the guard releases the head. The chain is not mutated here.
            let entry = unsafe { &*next };
            // SAFETY: address storage has its advertised length by contract;
            // copy_address additionally checks family, length and nullness.
            if let Some(address) = unsafe { copy_address(entry, port) } {
                addresses.push(address);
            }
            next = entry.ai_next;
        }
        Ok(addresses)
    }

    unsafe fn copy_address(entry: &libc::addrinfo, port: u16) -> Option<SocketAddr> {
        if entry.ai_addr.is_null() {
            return None;
        }
        match entry.ai_family {
            libc::AF_INET if entry.ai_addrlen as usize >= mem::size_of::<libc::sockaddr_in>() => {
                // SAFETY: the caller guarantees advertised storage, checked
                // above. read_unaligned does not assume sockaddr alignment.
                let address =
                    unsafe { ptr::read_unaligned(entry.ai_addr.cast::<libc::sockaddr_in>()) };
                (i32::from(address.sin_family) == libc::AF_INET).then(|| {
                    SocketAddr::V4(SocketAddrV4::new(
                        Ipv4Addr::from(address.sin_addr.s_addr.to_ne_bytes()),
                        port,
                    ))
                })
            }
            libc::AF_INET6 if entry.ai_addrlen as usize >= mem::size_of::<libc::sockaddr_in6>() => {
                // SAFETY: the same checked storage contract as IPv4 above.
                let address =
                    unsafe { ptr::read_unaligned(entry.ai_addr.cast::<libc::sockaddr_in6>()) };
                (i32::from(address.sin6_family) == libc::AF_INET6).then(|| {
                    SocketAddr::V6(SocketAddrV6::new(
                        Ipv6Addr::from(address.sin6_addr.s6_addr),
                        port,
                        address.sin6_flowinfo,
                        address.sin6_scope_id,
                    ))
                })
            }
            _ => None,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::resolver::{facts, numeric_address};
        use std::{
            cell::Cell,
            panic::{AssertUnwindSafe, catch_unwind},
            sync::{
                Arc,
                atomic::{AtomicUsize, Ordering},
                mpsc,
            },
        };

        fn empty_entry() -> libc::addrinfo {
            // SAFETY: null pointers and zero scalar fields are valid here.
            unsafe { mem::zeroed() }
        }

        fn ipv4() -> libc::sockaddr_in {
            libc::sockaddr_in {
                sin_family: libc::AF_INET as libc::sa_family_t,
                sin_port: 123u16.to_be(),
                sin_addr: libc::in_addr {
                    s_addr: u32::from_ne_bytes([127, 0, 0, 7]),
                },
                sin_zero: [0; 8],
            }
        }

        #[test]
        fn numeric_routes_preserve_ipv6_scope_without_a_dns_lookup() {
            assert_eq!(
                numeric_address("127.0.0.1", 5432).unwrap().unwrap(),
                "127.0.0.1:5432".parse::<SocketAddr>().unwrap()
            );
            assert_eq!(
                numeric_address("::1", 5432).unwrap().unwrap(),
                "[::1]:5432".parse::<SocketAddr>().unwrap()
            );
            assert_eq!(
                numeric_address("fe80::7%23", 5432).unwrap().unwrap(),
                SocketAddr::V6(SocketAddrV6::new("fe80::7".parse().unwrap(), 5432, 0, 23))
            );
            // This queries the real Linux loopback interface index, not DNS.
            let SocketAddr::V6(scoped) = numeric_address("fe80::7%lo", 5432).unwrap().unwrap()
            else {
                panic!("expected scoped IPv6 address");
            };
            assert_eq!(scoped.ip(), &"fe80::7".parse::<Ipv6Addr>().unwrap());
            assert_eq!(scoped.port(), 5432);
            assert!(scoped.scope_id() > 0);
            assert!(numeric_address("localhost", 5432).is_none());
            let error = numeric_address("fe80::7%4294967296", 5432)
                .unwrap()
                .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
            assert!(facts(&error).is_none());
            assert!(!format!("{error:?} {error}").contains("fe80::7"));
        }

        #[tokio::test]
        async fn native_numeric_ipv4_and_localhost_use_configured_port() {
            // Actual native getaddrinfo calls against inherited NSS/hosts.
            // This does not establish a controlled DNS failure or IPv6 setup.
            assert_eq!(
                lookup("127.0.0.1", 43210).await.unwrap(),
                vec!["127.0.0.1:43210".parse::<SocketAddr>().unwrap()]
            );
            let localhost = lookup("localhost", 43211).await.unwrap();
            assert!(!localhost.is_empty());
            assert!(localhost.iter().all(|address| address.port() == 43211));
            assert!(localhost.iter().all(|address| address.ip().is_loopback()));
        }

        #[tokio::test]
        async fn nul_is_an_input_error_without_resolver_facts() {
            let error = lookup("private\0host", 1).await.unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
            assert!(facts(&error).is_none());
            assert!(!format!("{error:?} {error}").contains("private"));
        }

        #[test]
        fn seam_retains_original_status_hints_and_redacts_diagnostics() {
            // Injected return codes verify storage, not actual DNS outcomes.
            for status in [
                libc::EAI_AGAIN,
                libc::EAI_NONAME,
                libc::EAI_MEMORY,
                i32::MAX,
            ] {
                let calls = Cell::new(0);
                // SAFETY: no successful chain is supplied or accessed.
                let error = unsafe {
                    lookup_with(
                        c"private-host",
                        5432,
                        |host, service, hints, result| {
                            calls.set(calls.get() + 1);
                            assert_eq!(host, c"private-host");
                            assert_eq!(service, c"0");
                            assert!(hints.ai_addr.is_null());
                            assert!(hints.ai_next.is_null());
                            assert!(hints.ai_canonname.is_null());
                            // A failure's out-pointer must not be followed or freed.
                            *result = ptr::dangling_mut();
                            status
                        },
                        |_| panic!("failed lookup has no owned chain"),
                        || panic!("non-system EAI has no errno"),
                    )
                }
                .unwrap_err();
                assert_eq!(calls.get(), 1);
                let observation = facts(&error).unwrap();
                assert_eq!(observation.eai, status);
                assert_eq!(observation.errno, None);
                assert_eq!(observation.flags, libc::AI_ADDRCONFIG);
                assert_eq!(observation.family, libc::AF_UNSPEC);
                assert_eq!(observation.socket_type, libc::SOCK_STREAM);
                assert_eq!(observation.protocol, 0);
                assert_eq!(observation.service, "0");
                assert_eq!(format!("{observation:?}"), "ResolverFacts");
                assert_eq!(
                    format!("{}", error.get_ref().unwrap()),
                    "resolver unavailable"
                );
                assert!(std::error::Error::source(error.get_ref().unwrap()).is_none());
                assert!(!format!("{error:?} {error}").contains("private-host"));
            }
            assert!(facts(&io::Error::from_raw_os_error(libc::EIO)).is_none());
        }

        #[test]
        fn seam_captures_worker_errno_only_for_eai_system() {
            // SAFETY: failure never supplies an owned chain. errno is set/read
            // on this same thread to exercise the original-operation capture.
            let error = unsafe {
                lookup_with(
                    c"private-host",
                    1,
                    |_, _, _, _| {
                        *libc::__errno_location() = libc::EACCES;
                        libc::EAI_SYSTEM
                    },
                    |_| panic!("failed lookup has no owned chain"),
                    || *libc::__errno_location(),
                )
            }
            .unwrap_err();
            assert_eq!(facts(&error).unwrap().eai, libc::EAI_SYSTEM);
            assert_eq!(facts(&error).unwrap().errno, Some(libc::EACCES));
            assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
            // The EAI status is not misrepresented as an OS errno.
            assert_eq!(error.raw_os_error(), None);
        }

        #[test]
        fn seam_copies_valid_addresses_in_order_with_duplicates_and_frees_once() {
            let mut v4 = ipv4();
            let mut v6 = libc::sockaddr_in6 {
                sin6_family: libc::AF_INET6 as libc::sa_family_t,
                sin6_port: 456u16.to_be(),
                sin6_flowinfo: 19,
                sin6_addr: libc::in6_addr { s6_addr: [7; 16] },
                sin6_scope_id: 23,
            };
            let mut wrong_family = ipv4();
            wrong_family.sin_family = libc::AF_INET6 as libc::sa_family_t;
            let mut entries: [libc::addrinfo; 8] = std::array::from_fn(|_| empty_entry());
            for entry in &mut entries {
                entry.ai_family = libc::AF_INET;
                entry.ai_addrlen = mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;
                entry.ai_addr = ptr::from_mut(&mut v4).cast();
            }
            entries[1].ai_family = libc::AF_INET6;
            entries[1].ai_addrlen = mem::size_of::<libc::sockaddr_in6>() as libc::socklen_t;
            entries[1].ai_addr = ptr::from_mut(&mut v6).cast();
            entries[3].ai_addr = ptr::null_mut();
            entries[4].ai_addrlen -= 1;
            entries[5].ai_family = libc::AF_UNIX;
            entries[6].ai_addr = ptr::from_mut(&mut wrong_family).cast();
            entries[7].ai_family = libc::AF_INET6;
            entries[7].ai_addrlen = 0;
            let head = entries.as_mut_ptr();
            for (index, entry) in entries.iter_mut().enumerate().take(7) {
                // SAFETY: index + 1 is inside the eight-entry fixture array.
                entry.ai_next = unsafe { head.add(index + 1) };
            }
            let frees = Cell::new(0);
            // SAFETY: all fixture entries/storage outlive lookup_with. The
            // fixture free callback observes ownership; stack storage stays live.
            let addresses = unsafe {
                lookup_with(
                    c"fixture",
                    54321,
                    |_, _, _, result| {
                        *result = head;
                        0
                    },
                    |result| {
                        assert_eq!(result, head);
                        frees.set(frees.get() + 1);
                    },
                    || panic!("successful lookup has no errno observation"),
                )
            }
            .unwrap();
            let expected4: SocketAddr = "127.0.0.7:54321".parse().unwrap();
            let expected6 =
                SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::from([7; 16]), 54321, 19, 23));
            assert_eq!(addresses, vec![expected4, expected6, expected4]);
            assert_eq!(frees.get(), 1);
        }

        #[test]
        fn seam_successful_empty_list_and_unwind_preserve_ownership() {
            // SAFETY: a successful null head has no storage to access/release.
            let addresses = unsafe {
                lookup_with(
                    c"fixture",
                    1,
                    |_, _, _, _| 0,
                    |_| panic!("null list has no allocation"),
                    || panic!("success has no errno observation"),
                )
            }
            .unwrap();
            assert!(addresses.is_empty());
            let frees = Cell::new(0);
            let mut entry = empty_entry();
            let head = ptr::from_mut(&mut entry);
            let result = catch_unwind(AssertUnwindSafe(|| {
                let _list = AddressList {
                    head,
                    free: Some(|result| {
                        assert_eq!(result, head);
                        frees.set(frees.get() + 1);
                    }),
                };
                panic!("controlled unwind after guard installation");
            }));
            assert!(result.is_err());
            assert_eq!(frees.get(), 1);
        }

        #[tokio::test]
        async fn seam_dropped_waiter_leaves_worker_cleanup_without_continuation() {
            let calls = Arc::new(AtomicUsize::new(0));
            let frees = Arc::new(AtomicUsize::new(0));
            let continuation = Arc::new(AtomicUsize::new(0));
            let (started_tx, started_rx) = tokio::sync::oneshot::channel();
            let (release_tx, release_rx) = mpsc::channel();
            let (freed_tx, freed_rx) = tokio::sync::oneshot::channel();
            let worker_calls = Arc::clone(&calls);
            let worker_frees = Arc::clone(&frees);
            let after_lookup = Arc::clone(&continuation);
            let waiter = tokio::spawn(async move {
                blocking_lookup(move || {
                    let mut address = ipv4();
                    let mut entry = empty_entry();
                    entry.ai_family = libc::AF_INET;
                    entry.ai_addrlen = mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;
                    entry.ai_addr = ptr::from_mut(&mut address).cast();
                    let head = ptr::from_mut(&mut entry);
                    // SAFETY: worker-owned fixture remains live until free.
                    unsafe {
                        lookup_with(
                            c"fixture",
                            1,
                            |_, _, _, result| {
                                worker_calls.fetch_add(1, Ordering::SeqCst);
                                started_tx.send(()).unwrap();
                                release_rx.recv().unwrap();
                                *result = head;
                                0
                            },
                            |result| {
                                assert_eq!(result, head);
                                worker_frees.fetch_add(1, Ordering::SeqCst);
                                freed_tx.send(()).unwrap();
                            },
                            || panic!("success has no errno observation"),
                        )
                    }
                })
                .await
                .unwrap();
                // Socket creation belongs after await, never to the worker.
                after_lookup.fetch_add(1, Ordering::SeqCst);
            });
            started_rx.await.unwrap();
            waiter.abort();
            assert!(waiter.await.unwrap_err().is_cancelled());
            release_tx.send(()).unwrap();
            freed_rx.await.unwrap();
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            assert_eq!(frees.load(Ordering::SeqCst), 1);
            assert_eq!(continuation.load(Ordering::SeqCst), 0);
        }

        #[tokio::test]
        async fn seam_worker_failure_does_not_claim_a_resolver_status() {
            let error = blocking_lookup(|| panic!("controlled worker panic"))
                .await
                .unwrap_err();
            assert!(facts(&error).is_none());
            assert_eq!(format!("{error}"), "resolver worker unavailable");
            assert!(!format!("{error:?}").contains("controlled worker panic"));
        }
    }
}
