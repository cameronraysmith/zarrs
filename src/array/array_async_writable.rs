use std::sync::Arc;

use futures::{stream::FuturesUnordered, StreamExt};

use crate::{
    array_subset::ArraySubset,
    storage::{AsyncWritableStorageTraits, StorageError, StorageHandle},
};

use super::{
    codec::{options::CodecOptions, ArrayCodecTraits},
    Array, ArrayError,
};

impl<TStorage: ?Sized + AsyncWritableStorageTraits + 'static> Array<TStorage> {
    /// Store metadata.
    ///
    /// # Errors
    /// Returns [`StorageError`] if there is an underlying store error.
    pub async fn async_store_metadata(&self) -> Result<(), StorageError> {
        let storage_handle = Arc::new(StorageHandle::new(self.storage.clone()));
        let storage_transformer = self
            .storage_transformers()
            .create_async_writable_transformer(storage_handle);
        crate::storage::async_create_array(&*storage_transformer, self.path(), &self.metadata())
            .await
    }

    /// Encode `chunk_bytes` and store at `chunk_indices`.
    ///
    /// A chunk composed entirely of the fill value will not be written to the store.
    ///
    /// # Errors
    /// Returns an [`ArrayError`] if
    ///  - `chunk_indices` are invalid,
    ///  - the length of `chunk_bytes` is not equal to the expected length (the product of the number of elements in the chunk and the data type size in bytes),
    ///  - there is a codec encoding error, or
    ///  - an underlying store error.
    pub async fn async_store_chunk_opt(
        &self,
        chunk_indices: &[u64],
        chunk_bytes: Vec<u8>,
        options: &CodecOptions,
    ) -> Result<(), ArrayError> {
        // Validation
        let chunk_array_representation = self.chunk_array_representation(chunk_indices)?;
        if chunk_bytes.len() as u64 != chunk_array_representation.size() {
            return Err(ArrayError::InvalidBytesInputSize(
                chunk_bytes.len(),
                chunk_array_representation.size(),
            ));
        }

        let all_fill_value = self.fill_value().equals_all(&chunk_bytes);
        if all_fill_value {
            self.async_erase_chunk(chunk_indices).await?;
            Ok(())
        } else {
            let storage_handle = Arc::new(StorageHandle::new(self.storage.clone()));
            let storage_transformer = self
                .storage_transformers()
                .create_async_writable_transformer(storage_handle);
            let chunk_encoded: Vec<u8> = self
                .codecs()
                .async_encode_opt(chunk_bytes, &chunk_array_representation, options)
                .await
                .map_err(ArrayError::CodecError)?;
            crate::storage::async_store_chunk(
                &*storage_transformer,
                self.path(),
                chunk_indices,
                self.chunk_key_encoding(),
                chunk_encoded.into(),
            )
            .await
            .map_err(ArrayError::StorageError)
        }
    }

    /// Encode `chunk_bytes` and store at `chunk_indices` (default options).
    #[allow(clippy::missing_panics_doc, clippy::missing_errors_doc)]
    pub async fn async_store_chunk(
        &self,
        chunk_indices: &[u64],
        chunk_bytes: Vec<u8>,
    ) -> Result<(), ArrayError> {
        self.async_store_chunk_opt(chunk_indices, chunk_bytes, &CodecOptions::default())
            .await
    }

    /// Encode `chunk_elements` and store at `chunk_indices`.
    ///
    /// A chunk composed entirely of the fill value will not be written to the store.
    ///
    /// # Errors
    /// Returns an [`ArrayError`] if
    ///  - the size of  `T` does not match the data type size, or
    ///  - a [`store_chunk`](Array::store_chunk) error condition is met.
    pub async fn async_store_chunk_elements_opt<T: bytemuck::Pod + Send + Sync>(
        &self,
        chunk_indices: &[u64],
        chunk_elements: Vec<T>,
        options: &CodecOptions,
    ) -> Result<(), ArrayError> {
        array_async_store_elements!(
            self,
            chunk_elements,
            async_store_chunk_opt(chunk_indices, chunk_elements, options)
        )
    }

    /// Encode `chunk_elements` and store at `chunk_indices` (default options).
    #[allow(clippy::missing_panics_doc, clippy::missing_errors_doc)]
    pub async fn async_store_chunk_elements<T: bytemuck::Pod + Send + Sync>(
        &self,
        chunk_indices: &[u64],
        chunk_elements: Vec<T>,
    ) -> Result<(), ArrayError> {
        self.async_store_chunk_elements_opt(chunk_indices, chunk_elements, &CodecOptions::default())
            .await
    }

    #[cfg(feature = "ndarray")]
    /// Encode `chunk_array` and store at `chunk_indices`.
    ///
    /// # Errors
    /// Returns an [`ArrayError`] if
    ///  - the size of `T` does not match the size of the data type,
    ///  - a [`store_chunk_elements`](Array::store_chunk_elements) error condition is met.
    #[allow(clippy::missing_panics_doc)]
    pub async fn async_store_chunk_ndarray_opt<T: bytemuck::Pod + Send + Sync>(
        &self,
        chunk_indices: &[u64],
        chunk_array: &ndarray::ArrayViewD<'_, T>,
        options: &CodecOptions,
    ) -> Result<(), ArrayError> {
        array_async_store_ndarray!(
            self,
            chunk_array,
            async_store_chunk_elements_opt(chunk_indices, chunk_array, options)
        )
    }

    #[cfg(feature = "ndarray")]
    /// Encode `chunk_array` and store at `chunk_indices` (default options).
    #[allow(clippy::missing_panics_doc, clippy::missing_errors_doc)]
    pub async fn async_store_chunk_ndarray<T: bytemuck::Pod + Send + Sync>(
        &self,
        chunk_indices: &[u64],
        chunk_array: &ndarray::ArrayViewD<'_, T>,
    ) -> Result<(), ArrayError> {
        self.async_store_chunk_ndarray_opt(chunk_indices, chunk_array, &CodecOptions::default())
            .await
    }

    /// Encode `chunks_bytes` and store at the chunks with indices represented by the `chunks` array subset.
    ///
    /// A chunk composed entirely of the fill value will not be written to the store.
    ///
    /// # Errors
    /// Returns an [`ArrayError`] if
    ///  - `chunks` are invalid,
    ///  - the length of `chunk_bytes` is not equal to the expected length (the product of the number of elements in the chunks and the data type size in bytes),
    ///  - there is a codec encoding error, or
    ///  - an underlying store error.
    #[allow(clippy::similar_names, clippy::missing_panics_doc)]
    pub async fn async_store_chunks_opt(
        &self,
        chunks: &ArraySubset,
        chunks_bytes: Vec<u8>,
        options: &CodecOptions,
    ) -> Result<(), ArrayError> {
        let num_chunks = chunks.num_elements_usize();
        if num_chunks == 1 {
            let chunk_indices = chunks.start();
            self.async_store_chunk_opt(chunk_indices, chunks_bytes, options)
                .await?;
        } else {
            let array_subset = self.chunks_subset(chunks)?;
            let element_size = self.data_type().size();
            let expected_size = element_size as u64 * array_subset.num_elements();
            if chunks_bytes.len() as u64 != expected_size {
                return Err(ArrayError::InvalidBytesInputSize(
                    chunks_bytes.len(),
                    expected_size,
                ));
            }

            let expected_size = array_subset.num_elements() * self.data_type().size() as u64;
            if chunks_bytes.len() as u64 != expected_size {
                return Err(ArrayError::InvalidBytesInputSize(
                    chunks_bytes.len(),
                    expected_size,
                ));
            }

            let element_size = self.data_type().size();

            let mut futures = chunks
                .indices()
                .into_iter()
                .map(|chunk_indices| {
                    let chunk_subset_in_array = unsafe {
                        self.chunk_grid()
                            .subset_unchecked(&chunk_indices, self.shape())
                            .unwrap()
                    };
                    let overlap = unsafe { array_subset.overlap_unchecked(&chunk_subset_in_array) };
                    let chunk_subset_in_array_subset =
                        unsafe { overlap.relative_to_unchecked(array_subset.start()) };
                    #[allow(clippy::similar_names)]
                    let chunk_bytes = unsafe {
                        chunk_subset_in_array_subset.extract_bytes_unchecked(
                            &chunks_bytes,
                            array_subset.shape(),
                            element_size,
                        )
                    };

                    debug_assert_eq!(
                        chunk_subset_in_array.num_elements(),
                        chunk_subset_in_array_subset.num_elements()
                    );

                    async move {
                        self.async_store_chunk_opt(&chunk_indices, chunk_bytes, options)
                            .await
                    }
                })
                .collect::<FuturesUnordered<_>>();
            while let Some(item) = futures.next().await {
                item?;
            }
        }
        Ok(())
    }

    /// Encode `chunks_bytes` and store at the chunks with indices represented by the `chunks` array subset (default options).
    #[allow(clippy::similar_names)]
    #[allow(clippy::missing_panics_doc, clippy::missing_errors_doc)]
    pub async fn async_store_chunks(
        &self,
        chunks: &ArraySubset,
        chunks_bytes: Vec<u8>,
    ) -> Result<(), ArrayError> {
        self.async_store_chunks_opt(chunks, chunks_bytes, &CodecOptions::default())
            .await
    }

    /// Variation of [`Array::async_store_chunks`] for elements with a known type.
    ///
    /// # Errors
    /// In addition to [`Array::async_store_chunks`] errors, returns an [`ArrayError`] if the size of `T` does not match the data type size.
    pub async fn async_store_chunks_elements_opt<T: bytemuck::Pod + Send + Sync>(
        &self,
        chunks: &ArraySubset,
        chunks_elements: Vec<T>,
        options: &CodecOptions,
    ) -> Result<(), ArrayError> {
        array_async_store_elements!(
            self,
            chunks_elements,
            async_store_chunks_opt(chunks, chunks_elements, options)
        )
    }

    /// Variation of [`Array::async_store_chunks`] for elements with a known type (default options).
    #[allow(clippy::missing_panics_doc, clippy::missing_errors_doc)]
    pub async fn async_store_chunks_elements<T: bytemuck::Pod + Send + Sync>(
        &self,
        chunks: &ArraySubset,
        chunks_elements: Vec<T>,
    ) -> Result<(), ArrayError> {
        self.async_store_chunks_elements_opt(chunks, chunks_elements, &CodecOptions::default())
            .await
    }

    #[cfg(feature = "ndarray")]
    /// Variation of [`Array::async_store_chunks`] for an [`ndarray::ArrayViewD`].
    ///
    /// # Errors
    /// In addition to [`Array::async_store_chunks`] errors, returns an [`ArrayError`] if the size of `T` does not match the data type size.
    pub async fn async_store_chunks_ndarray_opt<T: bytemuck::Pod + Send + Sync>(
        &self,
        chunks: &ArraySubset,
        chunks_array: &ndarray::ArrayViewD<'_, T>,
        options: &CodecOptions,
    ) -> Result<(), ArrayError> {
        array_async_store_ndarray!(
            self,
            chunks_array,
            async_store_chunks_elements_opt(chunks, chunks_array, options)
        )
    }

    #[cfg(feature = "ndarray")]
    /// Variation of [`Array::async_store_chunks`] for an [`ndarray::ArrayViewD`] (default options).
    #[allow(clippy::missing_panics_doc, clippy::missing_errors_doc)]
    pub async fn async_store_chunks_ndarray<T: bytemuck::Pod + Send + Sync>(
        &self,
        chunks: &ArraySubset,
        chunks_array: &ndarray::ArrayViewD<'_, T>,
    ) -> Result<(), ArrayError> {
        self.async_store_chunks_ndarray_opt(chunks, chunks_array, &CodecOptions::default())
            .await
    }

    /// Erase the chunk at `chunk_indices`.
    ///
    /// Succeeds if the key does not exist.
    ///
    /// # Errors
    /// Returns a [`StorageError`] if there is an underlying store error.
    pub async fn async_erase_chunk(&self, chunk_indices: &[u64]) -> Result<(), StorageError> {
        let storage_handle = Arc::new(StorageHandle::new(self.storage.clone()));
        let storage_transformer = self
            .storage_transformers()
            .create_async_writable_transformer(storage_handle);
        crate::storage::async_erase_chunk(
            &*storage_transformer,
            self.path(),
            chunk_indices,
            self.chunk_key_encoding(),
        )
        .await
    }

    /// Erase the chunks at the chunk indices enclosed by `chunks`.
    ///
    /// # Errors
    /// Returns a [`StorageError`] if there is an underlying store error.
    pub async fn async_erase_chunks(&self, chunks: &ArraySubset) -> Result<(), StorageError> {
        let storage_handle = Arc::new(StorageHandle::new(self.storage.clone()));
        let storage_transformer = self
            .storage_transformers()
            .create_async_writable_transformer(storage_handle);
        let mut futures = chunks
            .indices()
            .into_iter()
            .map(|chunk_indices| {
                let storage_transformer = storage_transformer.clone();
                async move {
                    crate::storage::async_erase_chunk(
                        &*storage_transformer,
                        self.path(),
                        &chunk_indices,
                        self.chunk_key_encoding(),
                    )
                    .await
                }
            })
            .collect::<FuturesUnordered<_>>();
        while let Some(item) = futures.next().await {
            item?;
        }
        Ok(())
    }
}
