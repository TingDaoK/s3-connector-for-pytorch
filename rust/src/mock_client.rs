use std::sync::Arc;

use mountpoint_s3_client::mock_client::{MockClient, MockClientConfig, MockObject};
use pyo3::{pyclass, pymethods};

use crate::MountpointS3Client;

#[derive(Clone)]
#[pyclass(name = "MockMountpointS3Client", module = "_s3dataset", frozen)]
pub struct PyMockClient {
    mock_client: Arc<MockClient>,
    #[pyo3(get)]
    pub(crate) throughput_target_gbps: f64,
    #[pyo3(get)]
    pub(crate) region: String,
    #[pyo3(get)]
    pub(crate) part_size: usize,
}

#[pymethods]
impl PyMockClient {
    #[new]
    #[pyo3(signature = (region, bucket, throughput_target_gbps = 10.0, part_size = 8 * 1024 * 1024))]
    pub fn new(
        region: String,
        bucket: String,
        throughput_target_gbps: f64,
        part_size: usize,
    ) -> PyMockClient {
        let config = MockClientConfig { bucket, part_size };
        let mock_client = Arc::new(MockClient::new(config));

        PyMockClient {
            mock_client,
            region,
            throughput_target_gbps,
            part_size,
        }
    }

    fn create_mocked_client(&self) -> MountpointS3Client {
        MountpointS3Client::new(
            self.region.clone(),
            self.throughput_target_gbps,
            self.part_size,
            self.mock_client.clone(),
        )
    }

    fn add_object(&self, key: String, data: Vec<u8>) {
        self.mock_client.add_object(&key, MockObject::from(data));
    }

    fn remove_object(&self, key: String) {
        self.mock_client.remove_object(&key);
    }
}
