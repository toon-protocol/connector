/// Fallible `[T; N] -> [U; N]`.
pub fn try_array_into_with<'a, T, E, U, F, const N: usize>(
    value: &'a [T; N],
    fun: F,
) -> Result<[U; N], E>
where
    T: 'a,
    F: Fn(&T) -> Result<U, E>,
    U: std::fmt::Debug,
{
    Ok(value
        .iter()
        .map(fun)
        .collect::<Result<Vec<_>, _>>()?
        .try_into()
        .unwrap()) // Never fail: `value` contains `N` elements
}
