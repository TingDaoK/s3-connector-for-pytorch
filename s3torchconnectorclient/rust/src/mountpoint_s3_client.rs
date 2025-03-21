/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * // SPDX-License-Identifier: BSD
 */

use mountpoint_s3_client::config::{
    AddressingStyle, EndpointConfig, S3ClientAuthConfig, S3ClientConfig,
};
use mountpoint_s3_client::config::{Allocator, Uri};
use mountpoint_s3_client::types::{GetObjectParams, HeadObjectParams, PutObjectParams};
use mountpoint_s3_client::user_agent::UserAgent;
use mountpoint_s3_client::{ObjectClient, S3CrtClient};
use mountpoint_s3_crt_sys::{aws_thread_join_all_managed, aws_thread_set_managed_join_timeout_ns};
use nix::unistd::Pid;
use pyo3::pyfunction;
use pyo3::types::PyTuple;
use pyo3::PyErr;
use pyo3::{pyclass, pymethods, Bound, PyRef, PyResult, ToPyObject};
use std::sync::Arc;

use crate::exception::python_exception;
use crate::get_object_stream::GetObjectStream;
use crate::list_object_stream::ListObjectStream;
use crate::mountpoint_s3_client_inner::{MountpointS3ClientInner, MountpointS3ClientInnerImpl};
use crate::put_object_stream::PutObjectStream;

use crate::build_info;
use crate::python_structs::py_head_object_result::PyHeadObjectResult;

#[pyclass(
    name = "MountpointS3Client",
    module = "s3torchconnectorclient._mountpoint_s3_client",
    frozen
)]
pub struct MountpointS3Client {
    pub(crate) client: Arc<dyn MountpointS3ClientInner + Send + Sync + 'static>,

    #[pyo3(get)]
    throughput_target_gbps: f64,
    #[pyo3(get)]
    region: String,
    #[pyo3(get)]
    part_size: usize,
    #[pyo3(get)]
    profile: Option<String>,
    #[pyo3(get)]
    unsigned: bool,
    #[pyo3(get)]
    force_path_style: bool,
    #[pyo3(get)]
    user_agent_prefix: String,
    #[pyo3(get)]
    endpoint: Option<String>,

    owner_pid: Pid,
}

/// Blocking call that waits for all managed threads to complete their join call.
///
/// This can only be called from the main thread or a non-managed thread.
/// By default the wait is unbounded, but that default can be overridden via
/// set_managed_join_timeout_ns()
///
/// Returns:
///     Result: Ok on success, Err with error message on failure
#[pyfunction]
pub fn join_all_managed_threads(timeout_ns: u64) -> PyResult<()> {
    unsafe {
        aws_thread_set_managed_join_timeout_ns(timeout_ns);
        let result = aws_thread_join_all_managed();
        if result != 0 {
            return Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                "Failed to join managed threads. Error code: {}",
                result
            )));
        }
    }
    Ok(())
}

#[pymethods]
impl MountpointS3Client {
    #[new]
    #[pyo3(signature = (region, user_agent_prefix="".to_string(), throughput_target_gbps=10.0, part_size=8*1024*1024, profile=None, unsigned=false, endpoint=None, force_path_style=false))]
    #[allow(clippy::too_many_arguments)]
    pub fn new_s3_client(
        region: String,
        user_agent_prefix: String,
        throughput_target_gbps: f64,
        part_size: usize,
        profile: Option<String>,
        unsigned: bool,
        endpoint: Option<String>,
        force_path_style: bool,
    ) -> PyResult<Self> {
        // TODO: Mountpoint has logic for guessing based on instance type. It may be worth having
        // similar logic if we want to exceed 10Gbps reading for larger instances

        let endpoint_str = endpoint.as_deref().unwrap_or("");
        let mut endpoint_config = if endpoint_str.is_empty() {
            EndpointConfig::new(&region)
        } else {
            EndpointConfig::new(&region)
                .endpoint(Uri::new_from_str(&Allocator::default(), endpoint_str).unwrap())
        };
        if force_path_style {
            endpoint_config = endpoint_config.addressing_style(AddressingStyle::Path);
        }
        let auth_config = auth_config(profile.as_deref(), unsigned);

        let user_agent_suffix =
            &format!("{}/{}", build_info::PACKAGE_NAME, build_info::FULL_VERSION);
        let mut user_agent_string = &format!("{} {}", &user_agent_prefix, &user_agent_suffix);
        if user_agent_prefix.ends_with(user_agent_suffix) {
            // If we unpickle a client, we should not append the suffix again
            user_agent_string = &user_agent_prefix;
        }

        let config = S3ClientConfig::new()
            .user_agent(UserAgent::new(Some(user_agent_string.to_owned())))
            .throughput_target_gbps(throughput_target_gbps)
            .part_size(part_size)
            .auth_config(auth_config)
            .endpoint_config(endpoint_config);
        let crt_client = Arc::new(S3CrtClient::new(config).map_err(python_exception)?);

        Ok(MountpointS3Client::new(
            region,
            user_agent_prefix.to_string(),
            throughput_target_gbps,
            part_size,
            profile,
            unsigned,
            force_path_style,
            crt_client,
            endpoint,
        ))
    }

    pub fn get_object(
        slf: PyRef<'_, Self>,
        bucket: String,
        key: String,
    ) -> PyResult<GetObjectStream> {
        let params = GetObjectParams::default();
        slf.client.get_object(slf.py(), bucket, key, params)
    }

    #[pyo3(signature = (bucket, prefix=String::from(""), delimiter=String::from(""), max_keys=1000))]
    pub fn list_objects(
        &self,
        bucket: String,
        prefix: String,
        delimiter: String,
        max_keys: usize,
    ) -> ListObjectStream {
        ListObjectStream::new(self.client.clone(), bucket, prefix, delimiter, max_keys)
    }

    #[pyo3(signature = (bucket, key, storage_class=None))]
    pub fn put_object(
        slf: PyRef<'_, Self>,
        bucket: String,
        key: String,
        storage_class: Option<String>,
    ) -> PyResult<PutObjectStream> {
        let mut params = PutObjectParams::default();
        params.storage_class = storage_class;

        slf.client.put_object(slf.py(), bucket, key, params)
    }

    pub fn head_object(
        slf: PyRef<'_, Self>,
        bucket: String,
        key: String,
    ) -> PyResult<PyHeadObjectResult> {
        let params = HeadObjectParams::default();
        slf.client.head_object(slf.py(), bucket, key, params)
    }

    pub fn delete_object(slf: PyRef<'_, Self>, bucket: String, key: String) -> PyResult<()> {
        slf.client.delete_object(slf.py(), bucket, key)
    }

    pub fn copy_object(
        slf: PyRef<'_, Self>,
        src_bucket: String,
        src_key: String,
        dst_bucket: String,
        dst_key: String,
    ) -> PyResult<()> {
        slf.client
            .copy_object(slf.py(), src_bucket, src_key, dst_bucket, dst_key)
    }

    pub fn __getnewargs__(slf: PyRef<'_, Self>) -> PyResult<Bound<'_, PyTuple>> {
        let py = slf.py();
        let state = [
            slf.region.to_object(py),
            slf.user_agent_prefix.to_object(py),
            slf.throughput_target_gbps.to_object(py),
            slf.part_size.to_object(py),
            slf.profile.to_object(py),
            slf.unsigned.to_object(py),
            slf.endpoint.to_object(py),
            slf.force_path_style.to_object(py),
        ];
        Ok(PyTuple::new_bound(py, state))
    }
}

#[allow(clippy::too_many_arguments)]
impl MountpointS3Client {
    pub(crate) fn new<Client>(
        region: String,
        user_agent_prefix: String,
        throughput_target_gbps: f64,
        part_size: usize,
        profile: Option<String>,
        // no_sign_request on mountpoint-s3-client
        unsigned: bool,
        force_path_style: bool,
        client: Arc<Client>,
        endpoint: Option<String>,
    ) -> Self
    where
        Client: ObjectClient + Sync + Send + 'static,
        <Client as ObjectClient>::GetObjectResponse: Unpin + Sync,
        <Client as ObjectClient>::PutObjectRequest: Sync,
    {
        Self {
            throughput_target_gbps,
            part_size,
            region,
            profile,
            unsigned,
            force_path_style,
            client: Arc::new(MountpointS3ClientInnerImpl::new(client)),
            user_agent_prefix,
            endpoint,
            owner_pid: nix::unistd::getpid(),
        }
    }
}

fn auth_config(profile: Option<&str>, unsigned: bool) -> S3ClientAuthConfig {
    if unsigned {
        S3ClientAuthConfig::NoSigning
    } else if let Some(profile_name) = profile {
        S3ClientAuthConfig::Profile(profile_name.to_string())
    } else {
        S3ClientAuthConfig::Default
    }
}

impl Drop for MountpointS3Client {
    fn drop(&mut self) {
        println!("DROP HAPPENS");
        if nix::unistd::getpid() != self.owner_pid {
            println!("HACK HAPPENS");
            // We don't want to try to deallocate a client on a different process after a fork, as
            // the threads the destructor is expecting to exist actually don't (they didn't survive
            // the fork). So we intentionally leak the inner client by bumping its reference count
            // and then forgetting it, so the reference count can never reach zero. It's a memory
            // leak, but not a big one in practice given how long we expect clients to live.
            std::mem::forget(Arc::clone(&self.client));
        }
    }
}
