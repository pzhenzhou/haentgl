pub mod sys {
    use std::env;
    use std::path::Path;
    use tracing::error;

    pub const V1_CPU_QUOTA_PATH: &str = "/sys/fs/cgroup/cpu/cpu.cfs_quota_us";
    pub const V1_CPU_PERIOD_PATH: &str = "/sys/fs/cgroup/cpu/cpu.cfs_period_us";
    pub const V2_CPU_LIMIT_PATH: &str = "/sys/fs/cgroup/cpu.max";

    pub const DEFAULT_CGROUP_MAX_INDICATOR: &str = "max";
    const CGROUP_ROOT_HIERARCYHY: &str = "/sys/fs/cgroup";
    const LINUX_OS: &str = "linux";
    const DOCKER_ENV_PATH: &str = "/.dockerenv";
    const CONTAINER_ENV: &str = "SQL_PROXY_CONTAINER";

    pub const CGROUP_V2_CONTROLLER_LIST_PATH: &str = "/sys/fs/cgroup/cgroup.controllers";

    pub const KUBERNETES_SECRETS_PATH: &str = "/var/run/secrets/kubernetes.io";

    const KUBERNETES_HOSTNAME_ENV: &str = "SQL_PROXY_POD_NAME";

    #[derive(Debug, Clone, Copy)]
    pub enum CollectResource {
        Cpu,
        Memory,
    }

    #[derive(Debug, Clone, Copy)]
    pub enum CGroupVersion {
        V1,
        V2,
    }

    pub fn parse_controller_enable_file_for_cgroup_v2(file_path: &str, collect_name: &str) -> bool {
        match fs_err::read_to_string(file_path) {
            Ok(controller_string) => {
                for controller in controller_string.split_whitespace() {
                    if controller.eq(collect_name) {
                        return true;
                    };
                }
                false
            }
            Err(_) => false,
        }
    }

    pub fn read_file_usize(file_path: &str) -> Result<usize, std::io::Error> {
        let content = fs_err::read_to_string(file_path)?;
        let limit_val = content.trim().parse::<usize>().map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("failed to parse, path: {file_path}, content: {content}, error: {e}"),
            )
        })?;
        Ok(limit_val)
    }

    pub fn is_linux() -> bool {
        env::consts::OS.eq(LINUX_OS)
    }

    #[cfg(unix)]
    #[inline]
    pub fn hostname() -> String {
        env::var(KUBERNETES_HOSTNAME_ENV).unwrap_or_else(|_e| {
            //warn!("Failed get env {KUBERNETES_HOSTNAME_ENV} cause by {e:?}");
            use libc::{c_char, sysconf, _SC_HOST_NAME_MAX};
            use std::os::unix::ffi::OsStringExt;
            // Get the maximum size of host names on this system, and account for the
            // trailing NUL byte.
            let hostname_max = unsafe { sysconf(_SC_HOST_NAME_MAX) };
            let mut buffer = vec![0; (hostname_max as usize) + 1];
            let status_code =
                unsafe { libc::gethostname(buffer.as_mut_ptr() as *mut c_char, buffer.len()) };
            if status_code != 0 {
                // There are no reasonable failures
                error!(
                    "Failed to get hostname {:?}",
                    std::io::Error::last_os_error()
                );
                "_NONE_HOSTNAME".to_string()
            } else {
                let end = buffer.iter().position(|&b| b == 0).unwrap_or(buffer.len());
                buffer.resize(end, 0);
                let hostname_os_string = std::ffi::OsString::from_vec(buffer);
                hostname_os_string.into_string().unwrap()
            }
        })
    }
    pub fn has_cgroup() -> bool {
        Path::new(CGROUP_ROOT_HIERARCYHY).is_dir()
    }

    pub fn is_container() -> bool {
        return env::var(CONTAINER_ENV).is_ok()
            || Path::new(DOCKER_ENV_PATH).exists()
            || Path::new(KUBERNETES_SECRETS_PATH).exists();
    }

    pub fn cgroup_version() -> CGroupVersion {
        if Path::new(CGROUP_V2_CONTROLLER_LIST_PATH).exists() {
            CGroupVersion::V2
        } else {
            CGroupVersion::V1
        }
    }

    pub fn controller_activated(
        collect_resource: CollectResource,
        cgroup_version: CGroupVersion,
    ) -> bool {
        let controller_name: &str = match collect_resource {
            CollectResource::Cpu => "cpu",
            CollectResource::Memory => "memory",
        };
        match cgroup_version {
            CGroupVersion::V1 => Path::new(CGROUP_ROOT_HIERARCYHY)
                .join(controller_name)
                .is_dir(),
            CGroupVersion::V2 => parse_controller_enable_file_for_cgroup_v2(
                CGROUP_V2_CONTROLLER_LIST_PATH,
                controller_name,
            ),
        }
    }
}

pub mod cpu {
    use crate::sys_utils::sys::*;
    use std::thread;
    use tracing::warn;

    pub fn get_total_cpu() -> f32 {
        if is_linux() || is_container() || has_cgroup() {
            return sys_cpu();
        }
        let cgroup_version = cgroup_version();

        if !controller_activated(CollectResource::Cpu, cgroup_version) {
            return sys_cpu();
        }

        get_container_cpu(cgroup_version).unwrap_or_else(|e| {
            warn!("Failed get cpu in Container. {:?}", e);
            sys_cpu()
        })
    }

    pub fn sys_cpu() -> f32 {
        match thread::available_parallelism() {
            Ok(available_parallelism) => available_parallelism.get() as f32,
            Err(e) => panic!("Failed to get THD available parallelism. cause by {e:?}"),
        }
    }

    fn get_container_cpu_limit_v2(limit_path: &str, max_value: f32) -> Result<f32, std::io::Error> {
        let cpu_limit_string = fs_err::read_to_string(limit_path)?;
        let cpu_data: Vec<&str> = cpu_limit_string.split_whitespace().collect();
        match cpu_data.get(0..2) {
            Some(cpu_data_values) => {
                if cpu_data_values[0] == DEFAULT_CGROUP_MAX_INDICATOR {
                    return Ok(max_value);
                }
                // parse_error(limit_path, &cpu_limit_string, e)
                let cpu_quota = cpu_data_values[0]
                    .parse::<usize>()
                    .map_err(|e| std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("failed to parse, path: {limit_path}, content: {:?}, error: {e}", &cpu_limit_string),
                    ))?;
                let cpu_period = cpu_data_values[1]
                    .parse::<usize>()
                    .map_err(|e| std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("failed to parse, path: {limit_path}, content: {:?}, error: {e}", &cpu_limit_string),
                    ))?;
                Ok((cpu_quota as f32) / (cpu_period as f32))
            }
            None => Err(
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("Invalid format in Cgroup CPU interface file, path: {limit_path}, content: {cpu_limit_string}"),
                )
            ),
        }
    }

    fn get_container_cpu_limit_v1(
        quota_path: &str,
        period_path: &str,
        max_value: f32,
    ) -> Result<f32, std::io::Error> {
        let content = std::fs::read_to_string(quota_path)?;
        if content.trim() == DEFAULT_CGROUP_MAX_INDICATOR {
            return Ok(max_value);
        }
        let cpu_quota = content
            .trim()
            .parse::<usize>()
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "not a number"))?;
        let cpu_period = read_file_usize(period_path)?;

        Ok((cpu_quota as f32) / (cpu_period as f32))
    }

    pub fn get_container_cpu(cgroup_version: CGroupVersion) -> Result<f32, std::io::Error> {
        let max_cpu = sys_cpu();
        match cgroup_version {
            CGroupVersion::V1 => {
                get_container_cpu_limit_v1(V1_CPU_QUOTA_PATH, V1_CPU_PERIOD_PATH, max_cpu)
            }
            CGroupVersion::V2 => get_container_cpu_limit_v2(V2_CPU_LIMIT_PATH, max_cpu),
        }
    }
}
