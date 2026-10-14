//! A box that lays its children out in lines: each at its natural width,
//! from the start, and a new line when the next does not fit. GTK has no
//! such box: a flow box lines its children up in columns.

use gtk::{glib, prelude::*, subclass::prelude::*};

/// The space between two children of a line and between two lines, in
/// pixels.
const SPACING: i32 = 6;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct Wrap;

    #[glib::object_subclass]
    impl ObjectSubclass for Wrap {
        const NAME: &'static str = "AddictWrap";
        type Type = super::Wrap;
        type ParentType = gtk::Widget;

        fn class_init(class: &mut Self::Class) {
            class.set_css_name("wrap");
        }
    }

    impl ObjectImpl for Wrap {
        fn dispose(&self) {
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for Wrap {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let children = children(self.obj().upcast_ref::<gtk::Widget>());
            if orientation == gtk::Orientation::Horizontal {
                // At the least as wide as the widest child; at best one line.
                let minimum = children
                    .iter()
                    .map(|child| child.measure(orientation, -1).0)
                    .max()
                    .unwrap_or(0);
                let natural = children
                    .iter()
                    .map(|child| child.measure(orientation, -1).1)
                    .sum::<i32>()
                    + SPACING * (children.len() as i32 - 1).max(0);
                (minimum, natural.max(minimum), -1, -1)
            } else {
                let height = lines(&children, for_size)
                    .iter()
                    .map(|line| line.height)
                    .sum::<i32>();
                let gaps = SPACING * (lines(&children, for_size).len() as i32 - 1).max(0);
                (height + gaps, height + gaps, -1, -1)
            }
        }

        fn size_allocate(&self, width: i32, _height: i32, _baseline: i32) {
            let children = children(self.obj().upcast_ref::<gtk::Widget>());
            let mut y = 0;
            for line in lines(&children, width) {
                let mut x = 0;
                for (child, child_width) in &line.children {
                    child.size_allocate(&gtk::Allocation::new(x, y, *child_width, line.height), -1);
                    x += child_width + SPACING;
                }
                y += line.height + SPACING;
            }
        }
    }
}

glib::wrapper! {
    /// See the module.
    pub struct Wrap(ObjectSubclass<imp::Wrap>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Wrap {
    pub fn new() -> Self {
        glib::Object::new()
    }

    /// Puts `child` after the last child.
    pub fn append(&self, child: &impl IsA<gtk::Widget>) {
        child.set_parent(self);
    }
}

impl Default for Wrap {
    fn default() -> Self {
        Self::new()
    }
}

/// One line: its children with their widths, and its height.
struct Line {
    children: Vec<(gtk::Widget, i32)>,
    height: i32,
}

/// The visible children of `wrap`, in order.
fn children(wrap: &impl IsA<gtk::Widget>) -> Vec<gtk::Widget> {
    let mut children = Vec::new();
    let mut next = wrap.first_child();
    while let Some(child) = next {
        next = child.next_sibling();
        if child.should_layout() {
            children.push(child);
        }
    }
    children
}

/// `children` in lines no wider than `width`; a child wider than that
/// stands alone on its line, at that width. Without a width, one line.
fn lines(children: &[gtk::Widget], width: i32) -> Vec<Line> {
    let mut lines: Vec<Line> = Vec::new();
    let mut used = 0;
    for child in children {
        let natural = child.measure(gtk::Orientation::Horizontal, -1).1;
        let child_width = if width < 0 {
            natural
        } else {
            natural.min(width)
        };
        let fits = lines
            .last()
            .is_some_and(|_| width < 0 || used + SPACING + child_width <= width);
        if !fits {
            lines.push(Line {
                children: Vec::new(),
                height: 0,
            });
            used = 0;
        } else {
            used += SPACING;
        }
        let height = child.measure(gtk::Orientation::Vertical, child_width).1;
        let line = lines.last_mut().expect("a line was pushed");
        line.children.push((child.clone(), child_width));
        line.height = line.height.max(height);
        used += child_width;
    }
    lines
}
