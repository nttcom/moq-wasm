macro_rules! wire_enum {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident: $repr:ident {
            $($(#[$variant_meta:meta])* $variant:ident = $value:literal),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[repr($repr)]
        $vis enum $name {
            $($(#[$variant_meta])* $variant = $value),+
        }

        impl TryFrom<$repr> for $name {
            type Error = anyhow::Error;

            fn try_from(value: $repr) -> anyhow::Result<Self> {
                match value {
                    $($value => Ok(Self::$variant),)+
                    _ => anyhow::bail!("unknown {} value {value:#x}", stringify!($name)),
                }
            }
        }

        impl From<$name> for $repr {
            fn from(value: $name) -> Self {
                value as $repr
            }
        }
    };
}

pub(crate) use wire_enum;

#[cfg(test)]
mod tests {
    wire_enum! {
        #[derive(Debug, PartialEq)]
        enum Sample: u8 {
            First = 0x1,
            Third = 0x3,
        }
    }

    #[test]
    fn try_from_maps_known_values_to_variants() {
        // Act / Assert
        assert_eq!(Sample::try_from(0x3).unwrap(), Sample::Third);
    }

    #[test]
    fn try_from_rejects_values_without_a_variant() {
        // Act
        let error = Sample::try_from(0x2).unwrap_err();

        // Assert
        assert_eq!(error.to_string(), "unknown Sample value 0x2");
    }

    #[test]
    fn from_returns_the_discriminant() {
        // Act / Assert
        assert_eq!(u8::from(Sample::First), 0x1);
    }
}
