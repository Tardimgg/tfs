import {Component, computed, model, resource} from '@angular/core';
import {RouterLink, RouterLinkActive, RouterModule, RouterOutlet} from '@angular/router';
import {File, NgxVoyageComponent} from 'ngx-voyage';
import {log} from '@angular-devkit/build-angular/src/builders/ssr-dev-server';

@Component({
  selector: 'app-root',
  imports: [RouterOutlet, RouterLink, RouterLinkActive, RouterModule],
  templateUrl: './app.component.html',
  styleUrl: './app.component.css'
})
export class AppComponent {

  title = 'tfs-front';
  // files: File[] = [{
  //   isDirectory: true,
  //   isFile: false,
  //   name: 'ngx-voyage',
  //   size: 1,
  //   isSymbolicLink: false,
  //   modifiedDate: new Date(),
  // }]


}

