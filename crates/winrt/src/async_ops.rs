//! IAsyncOperation<T> and IAsyncAction patterns for WinRT.
//!
//! WinRT is heavily async. These types model the asynchronous operation
//! patterns used throughout the Windows Runtime API surface.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use crate::activation::WinRtValue;

/// Async operation status (matches Windows.Foundation.AsyncStatus).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsyncStatus {
    /// The operation has been created but not yet started.
    Created = 0,
    /// The operation is running.
    Started = 1,
    /// The operation completed successfully.
    Completed = 2,
    /// The operation was canceled.
    Canceled = 3,
    /// The operation completed with an error.
    Error = 4,
}

/// IAsyncOperation<T>: an async operation that produces a result.
///
/// In real WinRT, this is a COM interface. Our implementation wraps
/// a result value that can be polled.
#[derive(Debug, Clone)]
pub struct AsyncOperation<T: Clone> {
    /// Unique operation ID.
    pub id: u64,
    /// Current status.
    pub status: AsyncStatus,
    /// Result value (available when status == Completed).
    pub result: Option<T>,
    /// Error info (available when status == Error).
    pub error: Option<AsyncError>,
    /// Progress value (0-100).
    pub progress: u32,
}

impl<T: Clone> AsyncOperation<T> {
    /// Create a new pending async operation.
    pub fn new(id: u64) -> Self {
        Self {
            id,
            status: AsyncStatus::Created,
            result: None,
            error: None,
            progress: 0,
        }
    }

    /// Create an already-completed operation with a result.
    pub fn completed(id: u64, result: T) -> Self {
        Self {
            id,
            status: AsyncStatus::Completed,
            result: Some(result),
            error: None,
            progress: 100,
        }
    }

    /// Create a failed operation.
    pub fn failed(id: u64, error: AsyncError) -> Self {
        Self {
            id,
            status: AsyncStatus::Error,
            result: None,
            error: Some(error),
            progress: 0,
        }
    }

    /// Start the operation.
    pub fn start(&mut self) {
        if self.status == AsyncStatus::Created {
            self.status = AsyncStatus::Started;
        }
    }

    /// Complete the operation with a result.
    pub fn complete(&mut self, result: T) {
        self.status = AsyncStatus::Completed;
        self.result = Some(result);
        self.progress = 100;
    }

    /// Fail the operation with an error.
    pub fn fail(&mut self, error: AsyncError) {
        self.status = AsyncStatus::Error;
        self.error = Some(error);
    }

    /// Cancel the operation.
    pub fn cancel(&mut self) {
        if self.status == AsyncStatus::Started || self.status == AsyncStatus::Created {
            self.status = AsyncStatus::Canceled;
        }
    }

    /// Get the result (blocks conceptually; in our implementation, returns
    /// immediately if completed).
    pub fn get_results(&self) -> Result<T, AsyncError> {
        match self.status {
            AsyncStatus::Completed => {
                self.result.clone().ok_or(AsyncError {
                    code: -1,
                    message: String::from("Result not available"),
                })
            }
            AsyncStatus::Error => Err(self.error.clone().unwrap_or(AsyncError {
                code: -1,
                message: String::from("Unknown error"),
            })),
            AsyncStatus::Canceled => Err(AsyncError {
                code: -2147418113, // E_ABORT
                message: String::from("Operation was canceled"),
            }),
            _ => Err(AsyncError {
                code: -2147483623, // E_ILLEGAL_METHOD_CALL
                message: String::from("Operation not yet completed"),
            }),
        }
    }

    /// Check if the operation is done (completed, failed, or canceled).
    pub fn is_done(&self) -> bool {
        matches!(
            self.status,
            AsyncStatus::Completed | AsyncStatus::Error | AsyncStatus::Canceled
        )
    }
}

/// IAsyncAction: an async operation that produces no result (void).
#[derive(Debug, Clone)]
pub struct AsyncAction {
    /// Unique operation ID.
    pub id: u64,
    /// Current status.
    pub status: AsyncStatus,
    /// Error info.
    pub error: Option<AsyncError>,
}

impl AsyncAction {
    /// Create a new pending async action.
    pub fn new(id: u64) -> Self {
        Self {
            id,
            status: AsyncStatus::Created,
            error: None,
        }
    }

    /// Create an already-completed action.
    pub fn completed(id: u64) -> Self {
        Self {
            id,
            status: AsyncStatus::Completed,
            error: None,
        }
    }

    /// Start the action.
    pub fn start(&mut self) {
        if self.status == AsyncStatus::Created {
            self.status = AsyncStatus::Started;
        }
    }

    /// Complete the action.
    pub fn complete(&mut self) {
        self.status = AsyncStatus::Completed;
    }

    /// Fail the action.
    pub fn fail(&mut self, error: AsyncError) {
        self.status = AsyncStatus::Error;
        self.error = Some(error);
    }

    /// Cancel the action.
    pub fn cancel(&mut self) {
        if self.status == AsyncStatus::Started || self.status == AsyncStatus::Created {
            self.status = AsyncStatus::Canceled;
        }
    }

    /// Get results (for void actions, just check for errors).
    pub fn get_results(&self) -> Result<(), AsyncError> {
        match self.status {
            AsyncStatus::Completed => Ok(()),
            AsyncStatus::Error => Err(self.error.clone().unwrap_or(AsyncError {
                code: -1,
                message: String::from("Unknown error"),
            })),
            AsyncStatus::Canceled => Err(AsyncError {
                code: -2147418113,
                message: String::from("Operation was canceled"),
            }),
            _ => Err(AsyncError {
                code: -2147483623,
                message: String::from("Operation not yet completed"),
            }),
        }
    }

    /// Check if the action is done.
    pub fn is_done(&self) -> bool {
        matches!(
            self.status,
            AsyncStatus::Completed | AsyncStatus::Error | AsyncStatus::Canceled
        )
    }
}

/// IAsyncOperationWithProgress<T, P>: async operation with progress reporting.
#[derive(Debug, Clone)]
pub struct AsyncOperationWithProgress<T: Clone, P: Clone> {
    /// Inner operation.
    pub inner: AsyncOperation<T>,
    /// Last reported progress value.
    pub progress_value: Option<P>,
}

impl<T: Clone, P: Clone> AsyncOperationWithProgress<T, P> {
    /// Create a new operation with progress.
    pub fn new(id: u64) -> Self {
        Self {
            inner: AsyncOperation::new(id),
            progress_value: None,
        }
    }

    /// Report progress.
    pub fn report_progress(&mut self, progress: P) {
        self.progress_value = Some(progress);
    }

    /// Complete with a result.
    pub fn complete(&mut self, result: T) {
        self.inner.complete(result);
    }
}

/// Error information for failed async operations.
#[derive(Debug, Clone)]
pub struct AsyncError {
    /// HRESULT error code.
    pub code: i32,
    /// Error message.
    pub message: String,
}

impl AsyncError {
    /// Create a new error.
    pub fn new(code: i32, message: &str) -> Self {
        Self {
            code,
            message: String::from(message),
        }
    }
}

impl core::fmt::Display for AsyncError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "HRESULT 0x{:08X}: {}", self.code as u32, self.message)
    }
}

/// Next operation ID counter.
static NEXT_OP_ID: spin::Mutex<u64> = spin::Mutex::new(1);

/// Allocate a unique operation ID.
pub fn alloc_op_id() -> u64 {
    let mut id = NEXT_OP_ID.lock();
    let op_id = *id;
    *id += 1;
    op_id
}

/// Create a completed IAsyncOperation<String>.
pub fn completed_string_op(value: &str) -> AsyncOperation<String> {
    AsyncOperation::completed(alloc_op_id(), String::from(value))
}

/// Create a completed IAsyncAction.
pub fn completed_action() -> AsyncAction {
    AsyncAction::completed(alloc_op_id())
}

/// Create a failed IAsyncAction.
pub fn failed_action(code: i32, message: &str) -> AsyncAction {
    let mut action = AsyncAction::new(alloc_op_id());
    action.fail(AsyncError::new(code, message));
    action
}
