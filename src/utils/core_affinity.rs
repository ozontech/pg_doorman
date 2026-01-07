//! This crate manages CPU affinities.

#[cfg(any(
    target_os = "android",
    target_os = "linux",
    target_os = "macos",
    target_os = "freebsd"
))]
extern crate libc;

#[cfg_attr(all(not(test), not(target_os = "macos")), allow(unused_extern_crates))]
extern crate num_cpus;

/// This function tries to retrieve information
/// on all the "cores" on which the current thread
/// is allowed to run.
pub fn get_core_ids() -> Option<Vec<CoreId>> {
    get_core_ids_helper()
}

/// This function tries to pin the current
/// thread to the specified core.
///
/// # Arguments
///
/// * core_id - ID of the core to pin
pub fn set_for_current(core_id: CoreId) -> bool {
    set_for_current_helper(core_id)
}

pub fn clear_for_current() -> bool {
    clear_for_current_helper()
}

/// This represents a CPU core.
#[repr(transparent)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CoreId {
    pub id: usize,
}

// Linux Section

#[cfg(any(target_os = "android", target_os = "linux"))]
#[inline]
fn get_core_ids_helper() -> Option<Vec<CoreId>> {
    linux::get_core_ids()
}

#[cfg(any(target_os = "android", target_os = "linux"))]
#[inline]
fn set_for_current_helper(core_id: CoreId) -> bool {
    linux::set_for_current(core_id)
}

#[cfg(any(target_os = "android", target_os = "linux"))]
#[inline]
fn clear_for_current_helper() -> bool {
    linux::clear_for_current()
}

#[cfg(any(target_os = "android", target_os = "linux"))]
#[allow(non_camel_case_types)]
mod linux {
    use std::mem;

    use libc::{cpu_set_t, sched_getaffinity, sched_setaffinity, CPU_ISSET, CPU_SET, CPU_SETSIZE};

    use super::CoreId;

    pub fn get_core_ids() -> Option<Vec<CoreId>> {
        if let Some(full_set) = get_affinity_mask() {
            let mut core_ids: Vec<CoreId> = Vec::new();

            for i in 0..CPU_SETSIZE as usize {
                if unsafe { CPU_ISSET(i, &full_set) } {
                    core_ids.push(CoreId { id: i });
                }
            }

            Some(core_ids)
        } else {
            None
        }
    }

    pub fn set_for_current(core_id: CoreId) -> bool {
        // Turn `core_id` into a `libc::cpu_set_t` with only
        // one core active.
        let mut set = new_cpu_set();

        unsafe { CPU_SET(core_id.id, &mut set) };

        // Set the current thread's core affinity.
        let res = unsafe {
            sched_setaffinity(
                0, // Defaults to current thread
                mem::size_of::<cpu_set_t>(),
                &set,
            )
        };
        res == 0
    }

    pub fn clear_for_current() -> bool {
        let mut set = new_cpu_set();
        for x in 0..1024 {
            unsafe { CPU_SET(x, &mut set) };
        }
        // Set the current thread's core affinity.
        let res = unsafe {
            sched_setaffinity(
                0, // Defaults to current thread
                mem::size_of::<cpu_set_t>(),
                &set,
            )
        };
        res == 0
    }

    fn get_affinity_mask() -> Option<cpu_set_t> {
        let mut set = new_cpu_set();

        // Try to get current core affinity mask.
        let result = unsafe {
            sched_getaffinity(
                0, // Defaults to current thread
                mem::size_of::<cpu_set_t>(),
                &mut set,
            )
        };

        if result == 0 {
            Some(set)
        } else {
            None
        }
    }

    fn new_cpu_set() -> cpu_set_t {
        unsafe { mem::zeroed::<cpu_set_t>() }
    }
}

// MacOS Section

#[cfg(target_os = "macos")]
#[inline]
fn get_core_ids_helper() -> Option<Vec<CoreId>> {
    macos::get_core_ids()
}

#[cfg(target_os = "macos")]
#[inline]
fn set_for_current_helper(core_id: CoreId) -> bool {
    macos::set_for_current(core_id)
}

#[cfg(target_os = "macos")]
#[inline]
fn clear_for_current_helper() -> bool {
    false
}

#[cfg(target_os = "macos")]
#[allow(non_camel_case_types)]
mod macos {
    use std::mem;

    use libc::{c_int, c_uint, pthread_self};

    use num_cpus;

    use super::CoreId;

    type kern_return_t = c_int;
    type integer_t = c_int;
    type natural_t = c_uint;
    type thread_t = c_uint;
    type thread_policy_flavor_t = natural_t;
    type mach_msg_type_number_t = natural_t;

    #[repr(C)]
    struct thread_affinity_policy_data_t {
        affinity_tag: integer_t,
    }

    type thread_policy_t = *mut thread_affinity_policy_data_t;

    const THREAD_AFFINITY_POLICY: thread_policy_flavor_t = 4;

    extern "C" {
        fn thread_policy_set(
            thread: thread_t,
            flavor: thread_policy_flavor_t,
            policy_info: thread_policy_t,
            count: mach_msg_type_number_t,
        ) -> kern_return_t;
    }

    pub fn get_core_ids() -> Option<Vec<CoreId>> {
        Some(
            (0..(num_cpus::get()))
                .map(|n| CoreId { id: n })
                .collect::<Vec<_>>(),
        )
    }

    pub fn set_for_current(core_id: CoreId) -> bool {
        let thread_affinity_policy_count: mach_msg_type_number_t =
            mem::size_of::<thread_affinity_policy_data_t>() as mach_msg_type_number_t
                / mem::size_of::<integer_t>() as mach_msg_type_number_t;

        let mut info = thread_affinity_policy_data_t {
            affinity_tag: core_id.id as integer_t,
        };

        let res = unsafe {
            thread_policy_set(
                pthread_self() as thread_t,
                THREAD_AFFINITY_POLICY,
                &mut info as thread_policy_t,
                thread_affinity_policy_count,
            )
        };
        res == 0
    }
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "windows",
    target_os = "macos",
    target_os = "freebsd"
)))]
#[inline]
fn set_for_current_helper(_core_id: CoreId) -> bool {
    false
}
