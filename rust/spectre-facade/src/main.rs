//! Emits a facade `Spectre.Console.Ansi.dll` exposing just `Spectre.Console.Color`.
//!
//! SPT dropped its Spectre.Console dependency, but the frozen 4.1.2 mod surface
//! has `Spectre.Console.Color` baked into `ISptLogger<T>`, `SptLogMessage`,
//! `ClientLogRequest` and `Watermark.Draw`. A compiled mod's IL carries a typeref
//! to that type's *defining* assembly, so the only thing that satisfies it is an
//! assembly of that name — hence a facade rather than an SPT-owned type.
//!
//! Note the assembly name: in Spectre.Console 0.57.2 the `Color` type lives in
//! `Spectre.Console.Ansi`, not `Spectre.Console`. Naming the facade after the
//! package instead of the defining assembly produces a DLL the runtime never
//! consults, which looks exactly like success.
//!
//! The colours are inert: the server accepts them and prints plain text. The
//! struct exists to keep signatures bindable, not to carry meaning.
//!
//! Known fidelity gaps, both cosmetic and neither fatal to a caller:
//!
//! - `FromInt32` returns Default instead of the xterm palette entry.
//! - `ToString()` is the inherited ValueType one, so it yields
//!   "Spectre.Console.Color" where the real type yields the colour name.
//!
//! Scope is deliberately `Color` only. Mods that called `AnsiConsole`, `Markup`
//! or `Style` directly still break — that surface was an ambient dependency of
//! the old Spectre package, never part of SPT's own contract.

use dotnetdll::prelude::*;

mod colors;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out_path = std::env::args()
        .nth(1)
        .ok_or("usage: spectre-facade <output-path>")?;

    let mut res = Resolution::new(Module::new("Spectre.Console.Ansi.dll"));
    res.assembly = Some(Assembly::new("Spectre.Console.Ansi"));

    // Roslyn resolves references by full identity, so the defaults (0.0.0.0, no
    // token) match nothing. Mirror the identity the real Spectre.Console.Ansi.dll
    // records for System.Runtime.
    let corlib = res.push_assembly_reference(ExternalAssemblyReference {
        version: Version {
            major: 10,
            minor: 0,
            build: 0,
            revision: 0,
        },
        public_key_or_token: Some(vec![0xb0, 0x3f, 0x5f, 0x7f, 0x11, 0xd5, 0x0a, 0x3a].into()),
        ..ExternalAssemblyReference::new("System.Runtime")
    });
    let value_type = res.push_type_reference(type_ref! { System.ValueType in #corlib });

    let color =
        res.push_type_definition(TypeDefinition::new(Some("Spectre.Console".into()), "Color"));
    res[color].set_extends(value_type);
    res[color].flags.sealed = true;
    res[color].flags.accessibility = TypeAccessibility::Public;

    let field_r = res.push_field(
        color,
        Field::instance(Accessibility::Private, "_r", ctype! { byte }),
    );
    let field_g = res.push_field(
        color,
        Field::instance(Accessibility::Private, "_g", ctype! { byte }),
    );
    let field_b = res.push_field(
        color,
        Field::instance(Accessibility::Private, "_b", ctype! { byte }),
    );

    let color_type: MethodType = BaseType::valuetype(color).into();
    let color_member_type: MemberType = BaseType::valuetype(color).into();

    // R/G/B are properties on the real Color, so a compiled mod emits
    // `callvirt get_R()`. Plain fields would not satisfy that callsite.
    for (name, field) in [("R", field_r), ("G", field_g), ("B", field_b)] {
        let prop = res.push_property(
            color,
            Property::new(false, name, Parameter::value(ctype! { byte })),
        );
        res.set_property_getter(
            prop,
            Method::new(
                Accessibility::Public,
                msig! { byte () },
                format!("get_{name}"),
                Some(body::Method::new(asm! {
                    LoadArgument 0;
                    load_field field;
                    Return;
                })),
            ),
        );
    }

    // A value-type ctor assigns through arg 0 (a managed pointer) and never
    // chains to a base ctor.
    let ctor = res.push_method(
        color,
        Method::constructor(
            Accessibility::Public,
            vec![
                Parameter::value(ctype! { byte }),
                Parameter::value(ctype! { byte }),
                Parameter::value(ctype! { byte }),
            ],
            Some(body::Method::new(asm! {
                LoadArgument 0;
                LoadArgument 1;
                store_field field_r;
                LoadArgument 0;
                LoadArgument 2;
                store_field field_g;
                LoadArgument 0;
                LoadArgument 3;
                store_field field_b;
                Return;
            })),
        ),
    );

    for (name, r, g, b) in colors::COLORS {
        let prop = res.push_property(
            color,
            Property::new(true, *name, Parameter::value(color_member_type.clone())),
        );
        res.set_property_getter(
            prop,
            Method::new(
                Accessibility::Public,
                msig! { static @color_type () },
                format!("get_{name}"),
                Some(body::Method::new(asm! {
                    LoadConstantInt32 *r as i32;
                    LoadConstantInt32 *g as i32;
                    LoadConstantInt32 *b as i32;
                    new_object ctor;
                    Return;
                })),
            ),
        );
    }

    // ponytail: returns Default rather than the real xterm palette entry. A
    // faithful lookup needs a 256-way switch or a static array in raw IL, and
    // nothing observes the value — SPT discards colours before rendering. Emit
    // the real table here if a mod is ever found reading RGB back off this.
    res.push_method(
        color,
        Method::new(
            Accessibility::Public,
            msig! { static @color_type (int) },
            "FromInt32",
            Some(body::Method::new(asm! {
                LoadConstantInt32 0;
                LoadConstantInt32 0;
                LoadConstantInt32 0;
                new_object ctor;
                Return;
            })),
        ),
    );

    let written = res.write(WriteOptions {
        is_32_bit: false,
        is_executable: false,
    })?;

    std::fs::write(&out_path, written)?;
    eprintln!(
        "spectre-facade: wrote {out_path} ({} colours)",
        colors::COLORS.len()
    );

    Ok(())
}
