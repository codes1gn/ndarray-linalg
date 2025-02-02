use crate::{error::*, layout::MatrixLayout, *};
use cauchy::*;
use num_traits::{ToPrimitive, Zero};

#[cfg_attr(doc, katexit::katexit)]
/// Solve linear equations using LU-decomposition
///
/// For a given matrix $A$, LU decomposition is described as $A = PLU$ where:
///
/// - $L$ is lower matrix
/// - $U$ is upper matrix
/// - $P$ is permutation matrix represented by [Pivot]
///
/// This is designed as two step computation according to LAPACK API:
///
/// 1. Factorize input matrix $A$ into $L$, $U$, and $P$.
/// 2. Solve linear equation $Ax = b$ or compute inverse matrix $A^{-1}$
///    using the output of LU decomposition.
///
pub trait Solve_: Scalar + Sized {
    /// Computes the LU decomposition of a general $m \times n$ matrix
    /// with partial pivoting with row interchanges.
    ///
    /// Output
    /// -------
    /// - $U$ and $L$ are stored in `a` after LU decomposition has succeeded.
    /// - $P$ is returned as [Pivot]
    ///
    /// Error
    /// ------
    /// - if the matrix is singular
    ///   - On this case, `return_code` in [Error::LapackComputationalFailure] means
    ///     `return_code`-th diagonal element of $U$ becomes zero.
    ///
    /// LAPACK correspondance
    /// ----------------------
    ///
    /// | f32    | f64    | c32    | c64    |
    /// |:-------|:-------|:-------|:-------|
    /// | sgetrf | dgetrf | cgetrf | zgetrf |
    ///
    fn lu(l: MatrixLayout, a: &mut [Self]) -> Result<Pivot>;

    /// Compute inverse matrix $A^{-1}$ from the output of LU-decomposition
    ///
    /// LAPACK correspondance
    /// ----------------------
    ///
    /// | f32    | f64    | c32    | c64    |
    /// |:-------|:-------|:-------|:-------|
    /// | sgetri | dgetri | cgetri | zgetri |
    ///
    fn inv(l: MatrixLayout, a: &mut [Self], p: &Pivot) -> Result<()>;

    /// Solve linear equations $Ax = b$ using the output of LU-decomposition
    ///
    /// LAPACK correspondance
    /// ----------------------
    ///
    /// | f32    | f64    | c32    | c64    |
    /// |:-------|:-------|:-------|:-------|
    /// | sgetrs | dgetrs | cgetrs | zgetrs |
    ///
    fn solve(l: MatrixLayout, t: Transpose, a: &[Self], p: &Pivot, b: &mut [Self]) -> Result<()>;
}

macro_rules! impl_solve {
    ($scalar:ty, $getrf:path, $getri:path, $getrs:path) => {
        impl Solve_ for $scalar {
            fn lu(l: MatrixLayout, a: &mut [Self]) -> Result<Pivot> {
                let (row, col) = l.size();
                assert_eq!(a.len() as i32, row * col);
                if row == 0 || col == 0 {
                    // Do nothing for empty matrix
                    return Ok(Vec::new());
                }
                let k = ::std::cmp::min(row, col);
                let mut ipiv = vec_uninit(k as usize);
                let mut info = 0;
                unsafe {
                    $getrf(
                        &l.lda(),
                        &l.len(),
                        AsPtr::as_mut_ptr(a),
                        &l.lda(),
                        AsPtr::as_mut_ptr(&mut ipiv),
                        &mut info,
                    )
                };
                info.as_lapack_result()?;
                let ipiv = unsafe { ipiv.assume_init() };
                Ok(ipiv)
            }

            fn inv(l: MatrixLayout, a: &mut [Self], ipiv: &Pivot) -> Result<()> {
                let (n, _) = l.size();
                if n == 0 {
                    // Do nothing for empty matrices.
                    return Ok(());
                }

                // calc work size
                let mut info = 0;
                let mut work_size = [Self::zero()];
                unsafe {
                    $getri(
                        &n,
                        AsPtr::as_mut_ptr(a),
                        &l.lda(),
                        ipiv.as_ptr(),
                        AsPtr::as_mut_ptr(&mut work_size),
                        &(-1),
                        &mut info,
                    )
                };
                info.as_lapack_result()?;

                // actual
                let lwork = work_size[0].to_usize().unwrap();
                let mut work: Vec<MaybeUninit<Self>> = vec_uninit(lwork);
                unsafe {
                    $getri(
                        &l.len(),
                        AsPtr::as_mut_ptr(a),
                        &l.lda(),
                        ipiv.as_ptr(),
                        AsPtr::as_mut_ptr(&mut work),
                        &(lwork as i32),
                        &mut info,
                    )
                };
                info.as_lapack_result()?;

                Ok(())
            }

            fn solve(
                l: MatrixLayout,
                t: Transpose,
                a: &[Self],
                ipiv: &Pivot,
                b: &mut [Self],
            ) -> Result<()> {
                // If the array has C layout, then it needs to be handled
                // specially, since LAPACK expects a Fortran-layout array.
                // Reinterpreting a C layout array as Fortran layout is
                // equivalent to transposing it. So, we can handle the "no
                // transpose" and "transpose" cases by swapping to "transpose"
                // or "no transpose", respectively. For the "Hermite" case, we
                // can take advantage of the following:
                //
                // ```text
                // A^H x = b
                // ⟺ conj(A^T) x = b
                // ⟺ conj(conj(A^T) x) = conj(b)
                // ⟺ conj(conj(A^T)) conj(x) = conj(b)
                // ⟺ A^T conj(x) = conj(b)
                // ```
                //
                // So, we can handle this case by switching to "no transpose"
                // (which is equivalent to transposing the array since it will
                // be reinterpreted as Fortran layout) and applying the
                // elementwise conjugate to `x` and `b`.
                let (t, conj) = match l {
                    MatrixLayout::C { .. } => match t {
                        Transpose::No => (Transpose::Transpose, false),
                        Transpose::Transpose => (Transpose::No, false),
                        Transpose::Hermite => (Transpose::No, true),
                    },
                    MatrixLayout::F { .. } => (t, false),
                };
                let (n, _) = l.size();
                let nrhs = 1;
                let ldb = l.lda();
                let mut info = 0;
                if conj {
                    for b_elem in &mut *b {
                        *b_elem = b_elem.conj();
                    }
                }
                unsafe {
                    $getrs(
                        t.as_ptr(),
                        &n,
                        &nrhs,
                        AsPtr::as_ptr(a),
                        &l.lda(),
                        ipiv.as_ptr(),
                        AsPtr::as_mut_ptr(b),
                        &ldb,
                        &mut info,
                    )
                };
                if conj {
                    for b_elem in &mut *b {
                        *b_elem = b_elem.conj();
                    }
                }
                info.as_lapack_result()?;
                Ok(())
            }
        }
    };
} // impl_solve!

impl_solve!(
    f64,
    lapack_sys::dgetrf_,
    lapack_sys::dgetri_,
    lapack_sys::dgetrs_
);
impl_solve!(
    f32,
    lapack_sys::sgetrf_,
    lapack_sys::sgetri_,
    lapack_sys::sgetrs_
);
impl_solve!(
    c64,
    lapack_sys::zgetrf_,
    lapack_sys::zgetri_,
    lapack_sys::zgetrs_
);
impl_solve!(
    c32,
    lapack_sys::cgetrf_,
    lapack_sys::cgetri_,
    lapack_sys::cgetrs_
);
