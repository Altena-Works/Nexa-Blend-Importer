//! Shared Blender-to-MMD geometry conversion. No pose decomposition is performed.

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoordinateError {
    InvalidUnits,
    NonFinite(&'static str),
    NonAffineObjectMatrix,
    SingularObjectMatrix,
    ZeroNormal,
    Float32Range(&'static str),
}

impl fmt::Display for CoordinateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUnits => {
                formatter.write_str("coordinate units must have a positive finite product")
            }
            Self::NonFinite(field) => write!(formatter, "non-finite {field}"),
            Self::NonAffineObjectMatrix => formatter.write_str("object matrix must be affine"),
            Self::SingularObjectMatrix => {
                formatter.write_str("object matrix is singular or numerically non-invertible")
            }
            Self::ZeroNormal => formatter.write_str("normal must have nonzero length"),
            Self::Float32Range(field) => write!(formatter, "{field} exceeds the float32 range"),
        }
    }
}

impl std::error::Error for CoordinateError {}

/// Converts original Blender object-local geometry to MMD geometry.
///
/// `object_matrix` is Blender's evaluated local-to-world affine matrix, stored as
/// rows (`matrix[row][column]`) and multiplying a column vector `[x, y, z, 1]`.
/// Its last row must be `[0, 0, 0, 1]`. Inputs must retain Blender axes and units;
/// do not pre-apply the axis swap or unit scale to them or to the matrix. For
/// geometry already in Blender world space, supply the identity object matrix.
///
/// Points use `k C(M p)`, with `C(x, y, z) = (x, z, y)` and
/// `k = units_per_meter * scene_unit_scale`, exactly once. Normals use the
/// inverse transpose of the object's linear part followed by `C` and unit-length
/// normalization. The positive uniform unit scale does not affect their direction.
#[derive(Clone, Debug)]
pub struct CoordinateTransform {
    object_matrix: [[f64; 4]; 4],
    normal_cofactors: [[f64; 3]; 3],
    determinant_sign: f64,
    scale: f64,
}

impl CoordinateTransform {
    pub fn new(
        units_per_meter: f64,
        scene_unit_scale: f64,
        object_matrix: [[f64; 4]; 4],
    ) -> Result<Self, CoordinateError> {
        let scale = units_per_meter * scene_unit_scale;
        if !units_per_meter.is_finite()
            || !scene_unit_scale.is_finite()
            || units_per_meter <= 0.0
            || scene_unit_scale <= 0.0
            || !scale.is_finite()
            || scale <= 0.0
        {
            return Err(CoordinateError::InvalidUnits);
        }
        if object_matrix
            .iter()
            .flatten()
            .any(|value| !value.is_finite())
        {
            return Err(CoordinateError::NonFinite("object matrix"));
        }
        if object_matrix[3] != [0.0, 0.0, 0.0, 1.0] {
            return Err(CoordinateError::NonAffineObjectMatrix);
        }

        // A positive common divisor preserves determinant parity and prevents
        // uniform very large/small object scales from overflowing the cofactors.
        let largest = object_matrix[..3]
            .iter()
            .flat_map(|row| &row[..3])
            .map(|value| value.abs())
            .fold(0.0_f64, f64::max);
        if largest == 0.0 {
            return Err(CoordinateError::SingularObjectMatrix);
        }
        let a: [[f64; 3]; 3] = std::array::from_fn(|row| {
            std::array::from_fn(|column| object_matrix[row][column] / largest)
        });
        let normal_cofactors = [
            [
                a[1][1] * a[2][2] - a[1][2] * a[2][1],
                a[1][2] * a[2][0] - a[1][0] * a[2][2],
                a[1][0] * a[2][1] - a[1][1] * a[2][0],
            ],
            [
                a[0][2] * a[2][1] - a[0][1] * a[2][2],
                a[0][0] * a[2][2] - a[0][2] * a[2][0],
                a[0][1] * a[2][0] - a[0][0] * a[2][1],
            ],
            [
                a[0][1] * a[1][2] - a[0][2] * a[1][1],
                a[0][2] * a[1][0] - a[0][0] * a[1][2],
                a[0][0] * a[1][1] - a[0][1] * a[1][0],
            ],
        ];
        let determinant = dot(a[0], normal_cofactors[0]);
        if determinant == 0.0 {
            return Err(CoordinateError::SingularObjectMatrix);
        }

        Ok(Self {
            object_matrix,
            normal_cofactors,
            determinant_sign: determinant.signum(),
            scale,
        })
    }

    /// Transforms an original Blender object-local point, including translation.
    pub fn point(&self, point: [f64; 3]) -> Result<[f32; 3], CoordinateError> {
        finite(point, "point")?;
        let world: [f64; 3] = std::array::from_fn(|row| {
            dot(
                [
                    self.object_matrix[row][0],
                    self.object_matrix[row][1],
                    self.object_matrix[row][2],
                ],
                point,
            ) + self.object_matrix[row][3]
        });
        to_float32(
            [
                world[0] * self.scale,
                world[2] * self.scale,
                world[1] * self.scale,
            ],
            "point result",
        )
    }

    /// Transforms an original Blender object-local normal; ignores translation.
    pub fn normal(&self, normal: [f64; 3]) -> Result<[f32; 3], CoordinateError> {
        let normal = normalize(normal)?;
        // Cofactors = det(A) * inverse-transpose(A). Magnitude cancels during
        // normalization, but the determinant sign must be removed explicitly.
        let transformed: [f64; 3] = std::array::from_fn(|row| {
            dot(self.normal_cofactors[row], normal) * self.determinant_sign
        });
        let normal = normalize([transformed[0], transformed[2], transformed[1]])?;
        to_float32(normal, "normal result")
    }

    /// Flips Blender UV V once; repeated/outside-[0, 1] UVs are not clamped.
    pub fn uv(&self, uv: [f64; 2]) -> Result<[f32; 2], CoordinateError> {
        finite(uv, "UV")?;
        to_float32([uv[0], 1.0 - uv[1]], "UV result")
    }

    /// Converts a source triangle's winding exactly once. `C` reflects axes;
    /// a negative object determinant cancels that reflection. Writers must not
    /// apply another winding reversal. Index bounds belong to packet validation.
    pub fn triangle(&self, [a, b, c]: [u32; 3]) -> [u32; 3] {
        if self.determinant_sign > 0.0 {
            [a, c, b]
        } else {
            [a, b, c]
        }
    }
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn finite<const N: usize>(values: [f64; N], field: &'static str) -> Result<(), CoordinateError> {
    if values.iter().all(|value| value.is_finite()) {
        Ok(())
    } else {
        Err(CoordinateError::NonFinite(field))
    }
}

fn normalize(normal: [f64; 3]) -> Result<[f64; 3], CoordinateError> {
    finite(normal, "normal")?;
    let largest = normal
        .iter()
        .map(|value| value.abs())
        .fold(0.0_f64, f64::max);
    if largest == 0.0 {
        return Err(CoordinateError::ZeroNormal);
    }
    let scaled = normal.map(|value| value / largest);
    let length = dot(scaled, scaled).sqrt();
    Ok(scaled.map(|value| value / length))
}

fn to_float32<const N: usize>(
    values: [f64; N],
    field: &'static str,
) -> Result<[f32; N], CoordinateError> {
    finite(values, field)?;
    if values.iter().any(|value| value.abs() > f64::from(f32::MAX)) {
        return Err(CoordinateError::Float32Range(field));
    }
    Ok(values.map(|value| value as f32))
}

#[cfg(test)]
mod tests {
    use super::*;

    const IDENTITY: [[f64; 4]; 4] = [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ];

    const ROTATED_NONUNIFORM: [[f64; 4]; 4] = [
        [0.0, -3.0, 0.0, 10.0],
        [2.0, 0.0, 0.0, -4.0],
        [0.0, 0.0, 4.0, 7.0],
        [0.0, 0.0, 0.0, 1.0],
    ];

    fn near(actual: [f32; 3], expected: [f64; 3]) {
        for (actual, expected) in actual.into_iter().zip(expected) {
            assert!(
                (f64::from(actual) - expected).abs() < 1e-7,
                "{actual} != {expected}"
            );
        }
    }

    #[test]
    fn point_includes_object_transform_and_scene_units_once() {
        let transform = CoordinateTransform::new(12.5, 0.02, ROTATED_NONUNIFORM).unwrap();
        // Object world point (4, -2, 19), then C and k = 0.25.
        assert_eq!(transform.point([1.0, 2.0, 3.0]).unwrap(), [1.0, 4.75, -0.5]);
    }

    #[test]
    fn normal_uses_independent_inverse_transpose_expectation() {
        let transform = CoordinateTransform::new(12.5, 0.02, ROTATED_NONUNIFORM).unwrap();
        // Rz(90) * diag(2, 3, 4) gives inverse-transpose normal
        // (-1/3, 1/2, 1/4). C and normalization give (-4, 3, 6)/sqrt(61).
        let length = 61.0_f64.sqrt();
        near(
            transform.normal([1.0, 1.0, 1.0]).unwrap(),
            [-4.0 / length, 3.0 / length, 6.0 / length],
        );
    }

    #[test]
    fn winding_and_normals_include_object_reflection_parity() {
        let positive = CoordinateTransform::new(1.0, 1.0, IDENTITY).unwrap();
        assert_eq!(positive.triangle([7, 8, 9]), [7, 9, 8]);
        let mut mirrored = IDENTITY;
        mirrored[0][0] = -1.0;
        let negative = CoordinateTransform::new(1.0, 1.0, mirrored).unwrap();
        assert_eq!(negative.triangle([7, 8, 9]), [7, 8, 9]);
        assert_eq!(negative.normal([1.0, 0.0, 0.0]).unwrap(), [-1.0, 0.0, 0.0]);
        mirrored[1][1] = -1.0;
        let twice_mirrored = CoordinateTransform::new(1.0, 1.0, mirrored).unwrap();
        assert_eq!(twice_mirrored.triangle([7, 8, 9]), [7, 9, 8]);
    }

    #[test]
    fn rejects_invalid_units_and_object_matrices() {
        for (units, scene_scale) in [
            (0.0, 1.0),
            (-1.0, -1.0),
            (f64::NAN, 1.0),
            (1.0, f64::INFINITY),
            (f64::MAX, 2.0),
            (f64::MIN_POSITIVE, f64::MIN_POSITIVE),
        ] {
            assert_eq!(
                CoordinateTransform::new(units, scene_scale, IDENTITY).unwrap_err(),
                CoordinateError::InvalidUnits
            );
        }
        let mut matrix = IDENTITY;
        matrix[1] = matrix[0];
        assert_eq!(
            CoordinateTransform::new(1.0, 1.0, matrix).unwrap_err(),
            CoordinateError::SingularObjectMatrix
        );
        matrix = IDENTITY;
        matrix[0][0] = f64::INFINITY;
        assert_eq!(
            CoordinateTransform::new(1.0, 1.0, matrix).unwrap_err(),
            CoordinateError::NonFinite("object matrix")
        );
        matrix = IDENTITY;
        matrix[3][0] = 0.1;
        assert_eq!(
            CoordinateTransform::new(1.0, 1.0, matrix).unwrap_err(),
            CoordinateError::NonAffineObjectMatrix
        );
    }

    #[test]
    fn rejects_nonfinite_zero_and_out_of_range_geometry() {
        let transform = CoordinateTransform::new(2.0, 1.0, IDENTITY).unwrap();
        assert_eq!(
            transform.point([f64::NAN, 0.0, 0.0]),
            Err(CoordinateError::NonFinite("point"))
        );
        assert_eq!(
            transform.point([f64::MAX, 0.0, 0.0]),
            Err(CoordinateError::NonFinite("point result"))
        );
        assert_eq!(
            transform.point([f64::from(f32::MAX), 0.0, 0.0]),
            Err(CoordinateError::Float32Range("point result"))
        );
        assert_eq!(transform.normal([0.0; 3]), Err(CoordinateError::ZeroNormal));
        assert_eq!(
            transform.normal([0.0, f64::INFINITY, 0.0]),
            Err(CoordinateError::NonFinite("normal"))
        );
        assert_eq!(
            transform.uv([0.0, f64::NAN]),
            Err(CoordinateError::NonFinite("UV"))
        );
        assert_eq!(
            transform.uv([0.0, f64::MAX]),
            Err(CoordinateError::Float32Range("UV result"))
        );
    }

    #[test]
    fn uv_flips_v_without_clamping() {
        let transform = CoordinateTransform::new(1.0, 1.0, IDENTITY).unwrap();
        assert_eq!(transform.uv([0.25, 0.2]).unwrap(), [0.25, 0.8]);
        assert_eq!(transform.uv([-0.25, 1.5]).unwrap(), [-0.25, -0.5]);
    }
}
