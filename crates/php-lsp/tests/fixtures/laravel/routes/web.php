<?php

use Illuminate\Support\Facades\Route;

Route::get('/', fn () => view('welcome'))->name('home');

Route::name('users.')->prefix('users')->group(function () {
    Route::get('/{id}', 'UserController@show')->name('show');
});

Route::resource('photos', 'PhotoController')->only(['index', 'show']);
