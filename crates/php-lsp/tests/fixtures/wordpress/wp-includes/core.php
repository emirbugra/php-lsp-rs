<?php
function wp_boot() {
    do_action('init');
    do_action('wp_loaded');
    $title = apply_filters('the_title', 'Hello', 1);
}
