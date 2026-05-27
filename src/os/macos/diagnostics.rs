//! macOS thread sampling via Mach `task_threads` + `thread_info`.

use crate::os::diagnostics::ThreadSample;

pub fn supports_thread_sampling_impl() -> bool {
    true
}

pub fn sample_threads_impl() -> Vec<ThreadSample> {
    use std::mem;
    use std::ptr;

    type MachPort = u32;
    type KernReturn = i32;
    const KERN_SUCCESS: KernReturn = 0;
    const THREAD_BASIC_INFO: u32 = 3;
    const THREAD_BASIC_INFO_COUNT: u32 =
        (mem::size_of::<ThreadBasicInfo>() / mem::size_of::<i32>()) as u32;

    #[repr(C)]
    #[derive(Default)]
    struct TimeValue {
        seconds: i32,
        microseconds: i32,
    }

    #[repr(C)]
    #[derive(Default)]
    struct ThreadBasicInfo {
        user_time: TimeValue,
        system_time: TimeValue,
        cpu_usage: i32,
        policy: i32,
        run_state: i32,
        flags: i32,
        suspend_count: i32,
        sleep_time: i32,
    }

    extern "C" {
        // Use mach_task_self_ directly — the cached task port, avoids leaking send rights.
        static mach_task_self_: MachPort;
        fn task_threads(
            task: MachPort,
            thread_list: *mut *mut MachPort,
            thread_count: *mut u32,
        ) -> KernReturn;
        fn thread_info(
            thread: MachPort,
            flavor: u32,
            info: *mut i32,
            count: *mut u32,
        ) -> KernReturn;
        fn vm_deallocate(task: MachPort, address: usize, size: usize) -> KernReturn;
        fn mach_port_deallocate(task: MachPort, name: MachPort) -> KernReturn;
    }

    fn time_value_to_ms(tv: &TimeValue) -> f64 {
        tv.seconds as f64 * 1000.0 + tv.microseconds as f64 / 1000.0
    }

    let mut threads: Vec<ThreadSample> = Vec::new();

    unsafe {
        let task = mach_task_self_;
        let mut thread_list: *mut MachPort = ptr::null_mut();
        let mut thread_count: u32 = 0;

        if task_threads(task, &mut thread_list, &mut thread_count) != KERN_SUCCESS {
            return threads;
        }

        for i in 0..thread_count as isize {
            let thread_port = *thread_list.offset(i);
            let mut info: ThreadBasicInfo = mem::zeroed();
            let mut count = THREAD_BASIC_INFO_COUNT;

            if thread_info(
                thread_port,
                THREAD_BASIC_INFO,
                &mut info as *mut ThreadBasicInfo as *mut i32,
                &mut count,
            ) == KERN_SUCCESS
            {
                let user_ms = time_value_to_ms(&info.user_time);
                let kernel_ms = time_value_to_ms(&info.system_time);
                threads.push(ThreadSample {
                    id: thread_port,
                    total_ms: user_ms + kernel_ms,
                    user_ms,
                    kernel_ms,
                    name: String::new(), // Mach doesn't expose names cheaply; leave blank
                });
            }

            // Drop the send right we got from task_threads.
            mach_port_deallocate(task, thread_port);
        }

        // Free the thread list buffer.
        vm_deallocate(
            task,
            thread_list as usize,
            thread_count as usize * mem::size_of::<MachPort>(),
        );
    }

    threads
}
